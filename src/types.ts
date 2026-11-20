export type EngineState = "stopped" | "starting" | "running" | "restarting" | "error";

export interface Snapshot {
  state: EngineState;
  inputRmsDb: number;
  inputPeakDb: number;
  outputRmsDb: number;
  outputPeakDb: number;
  clip: boolean;
  noiseRemovedDb: number;
  procMs: number;
  pipelineMs: number;
  aiActive: boolean;
  dspActive: boolean;
  agcDb: number;
  vad: number;
  message: string | null;
}

export interface DevInfo {
  name: string;
  isDefault: boolean;
}

export interface DeviceLists {
  inputs: DevInfo[];
  outputs: DevInfo[];
}

export interface DeviceSelection {
  input: string | null;
  monitor: string | null;
  virtualOut: string | null;
  bufferFrames: number;
}

export interface Settings {
  inputDevice: string | null;
  monitorDevice: string | null;
  virtualOutput: string | null;
  noiseStrength: number;
  studioEnhance: boolean;
  preset: string;
  monitorEnabled: boolean;
  monitorVolume: number;
  autoGain: boolean;
  modeEnhanced: boolean;
  startWithWindows: boolean;
  minimizeToTray: boolean;
  launchMinimized: boolean;
  quality: string;
  micEq: EqConfig;
  speakerEq: EqConfig;
}

export interface UiState {
  snapshot: Snapshot | null;
  settings: Settings;
  selection: DeviceSelection;
  devices: DeviceLists;
}

export interface EqConfig { enabled: boolean; gains: number[]; preamp: number; }
