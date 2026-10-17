using System.Text.Json.Serialization;

namespace NeoBoxMonitor;

public class SensorReading
{
    [JsonPropertyName("hardware")]
    public string Hardware { get; set; } = string.Empty;

    [JsonPropertyName("hardwareType")]
    public string HardwareType { get; set; } = string.Empty;

    [JsonPropertyName("subHardware")]
    public string? SubHardware { get; set; }

    [JsonPropertyName("name")]
    public string Name { get; set; } = string.Empty;

    [JsonPropertyName("sensorType")]
    public string SensorType { get; set; } = string.Empty;

    [JsonPropertyName("value")]
    public float Value { get; set; }

    [JsonPropertyName("unit")]
    public string? Unit { get; set; }
}

public class SensorsResponse
{
    [JsonPropertyName("updatedAt")]
    public string UpdatedAt { get; set; } = string.Empty;

    [JsonPropertyName("sensors")]
    public List<SensorReading> Sensors { get; set; } = new();
}

public class ReadCommand
{
    [JsonPropertyName("cmd")]
    public string Cmd { get; set; } = string.Empty;
}