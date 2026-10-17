using System.Text.Json;
using System.Text.Json.Serialization;
using LibreHardwareMonitor.Hardware;
using System.Runtime.ExceptionServices;

namespace NeoBoxMonitor;

class Program
{
    private static Computer? _computer;
    private static readonly SensorVisitor _visitor = new(_ => { });

    // JSON 兜底：允许 NaN / Infinity 序列化为字面量，避免整体序列化崩溃
    private static readonly JsonSerializerOptions _jsonOptions = new()
    {
        NumberHandling = JsonNumberHandling.AllowNamedFloatingPointLiterals
    };

    static int Main(string[] args)
    {
        // 捕获非托管崩溃（AccessViolationException 等 SEH 异常）
        AppDomain.CurrentDomain.FirstChanceException += (_, e) =>
        {
            if (e.Exception is AccessViolationException || e.Exception is NullReferenceException)
            {
                Console.Error.WriteLine($"[NeoBoxMonitor] 首次触发异常: {e.Exception.GetType().Name}: {e.Exception.Message}");
            }
        };

        try
        {
            _computer = new Computer
            {
                IsCpuEnabled = true,
                IsGpuEnabled = true,
                IsMemoryEnabled = true,
                IsMotherboardEnabled = true,
                IsStorageEnabled = true,
                IsNetworkEnabled = false,
                IsControllerEnabled = false,
                IsPsuEnabled = false,
            };

            // 将 Computer.Open() 放在单独线程中，设置 15 秒超时
            // 【注意】这是安全网，不是核心修复
            // 核心修复是确保 PawnIO 在 Open() 之前已安装
            var openTask = Task.Run(() =>
            {
                try
                {
                    _computer.Open();
                    SafeAccept(_computer);
                    Console.Error.WriteLine("[NeoBoxMonitor] Computer.Open() 完成");
                }
                catch (Exception ex)
                {
                    Console.Error.WriteLine($"[NeoBoxMonitor] 硬件初始化异常: {ex.GetType().Name}: {ex.Message}");
                }
            });

            if (!openTask.Wait(TimeSpan.FromSeconds(15)))
            {
                Console.Error.WriteLine("[NeoBoxMonitor] Computer.Open() 超时(15s)，PawnIO 可能在当前硬件上不兼容");
                // 进程仍然存活，已初始化的硬件可正常读取
            }

            var input = Console.In;
            var output = Console.Out;

            while (true)
            {
                var line = input.ReadLine();
                if (line == null)
                    break; // parent closed stdin

                line = line.Trim();
                if (line.Length == 0)
                    continue;

                try
                {
                    var cmd = JsonSerializer.Deserialize<ReadCommand>(line);
                    if (cmd?.Cmd == "read")
                    {
                        SafeUpdateAndCollect(output);
                    }
                    else if (cmd?.Cmd == "exit")
                    {
                        break;
                    }
                }
                catch (JsonException)
                {
                    // ignore malformed input
                }
            }
        }
        catch (Exception ex)
        {
            var error = JsonSerializer.Serialize(new { error = $"{ex.GetType().Name}: {ex.Message}" });
            try { Console.WriteLine(error); Console.Out.Flush(); } catch { }
            return 1;
        }
        finally
        {
            try { _computer?.Close(); } catch { }
        }

        return 0;
    }

    /// <summary>
    /// 安全执行 Accept：某些硬件在 Accept 时可能触发 AccessViolation
    /// </summary>
    static void SafeAccept(Computer computer)
    {
        try
        {
            computer.Accept(_visitor);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[NeoBoxMonitor] Accept 异常: {ex.GetType().Name}: {ex.Message}");
        }
    }

    /// <summary>
    /// 安全执行 Update + Collect，任何硬件崩溃都不会拖垮进程
    /// </summary>
    static void SafeUpdateAndCollect(TextWriter output)
    {
        // 首次：暴力更新
        UpdateHardware(_computer!.Hardware);

        // 第二次 Accept（有时 Accept 比 Update 更容易触发崩溃，改用 SafeAccept）
        SafeAccept(_computer);

        var sensors = new List<SensorReading>();
        SafeCollectSensors(_computer.Hardware, sensors);

        var response = new SensorsResponse
        {
            UpdatedAt = DateTime.UtcNow.ToString("o"),
            Sensors = sensors
        };

        var json = JsonSerializer.Serialize(response, _jsonOptions);
        output.WriteLine(json);
        output.Flush();
    }

    static void CollectSensors(IEnumerable<IHardware> hardwareList, List<SensorReading> sensors)
    {
        foreach (var hw in hardwareList)
        {
            if (hw.SubHardware.Length > 0)
            {
                CollectSensors(hw.SubHardware, sensors);
            }

            foreach (var sensor in hw.Sensors)
            {
                if (!sensor.Value.HasValue)
                    continue;

                var value = sensor.Value.Value;
                // 过滤 NaN / Infinity / -Infinity：
                // 这些值无法被 JSON 表达（会导致 JsonSerializer 抛异常），且对前端展示无意义
                // 注：net48 上没有 float.IsFinite，用 IsNaN/IsInfinity 组合判断
                if (float.IsNaN(value) || float.IsInfinity(value))
                    continue;

                var reading = new SensorReading
                {
                    Hardware = hw.Name,
                    HardwareType = hw.HardwareType.ToString(),
                    SubHardware = hw.SubHardware.Length > 0 ? hw.Name : null,
                    Name = sensor.Name,
                    SensorType = sensor.SensorType.ToString(),
                    Value = value,
                    Unit = GetUnit(sensor.SensorType)
                };

                sensors.Add(reading);
            }
        }
    }

    /// <summary>
    /// 安全遍历传感器：将 CollectSensors 整体包在 try/catch 中，
    /// 防止某个传感器枚举时触发 AccessViolationException 导致进程退出
    /// </summary>
    static void SafeCollectSensors(IEnumerable<IHardware> hardwareList, List<SensorReading> sensors)
    {
        try
        {
            CollectSensors(hardwareList, sensors);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[NeoBoxMonitor] CollectSensors 异常: {ex.GetType().Name}: {ex.Message}");
        }
    }

    static void UpdateHardware(IEnumerable<IHardware> hardwareList)
    {
        foreach (var hw in hardwareList)
        {
            try
            {
                hw.Update();
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"[NeoBoxMonitor] 硬件 \"{hw.Name}\" ({hw.HardwareType}) 更新失败: {ex.GetType().Name}: {ex.Message}");
            }

            if (hw.SubHardware.Length > 0)
            {
                try
                {
                    UpdateHardware(hw.SubHardware);
                }
                catch (Exception ex)
                {
                    Console.Error.WriteLine($"[NeoBoxMonitor] 子硬件 \"{hw.Name}\" 更新失败: {ex.GetType().Name}: {ex.Message}");
                }
            }
        }
    }

    static string? GetUnit(SensorType type)
    {
        return type switch
        {
            SensorType.Voltage => "V",
            SensorType.Clock => "MHz",
            SensorType.Temperature => "°C",
            SensorType.Load => "%",
            SensorType.Fan => "RPM",
            SensorType.Flow => "L/h",
            SensorType.Control => "%",
            SensorType.Level => "%",
            SensorType.Power => "W",
            SensorType.Data => "GB",
            SensorType.SmallData => "MB",
            SensorType.Factor => "",
            SensorType.Frequency => "Hz",
            SensorType.Throughput => "B/s",
            SensorType.Current => "A",
            SensorType.Energy => "mWh",
            SensorType.Noise => "dBA",
            SensorType.Humidity => "%",
            SensorType.TimeSpan => "s",
            _ => null
        };
    }
}
