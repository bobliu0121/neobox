export type SensorReading = {
  hardware: string;
  hardwareType: string;
  subHardware?: string;
  name: string;
  sensorType: string;
  value: number;
  unit?: string;
};

export type SensorsResponse = {
  updatedAt: string;
  sensors: SensorReading[];
};
