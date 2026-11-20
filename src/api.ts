import { invoke } from "@tauri-apps/api/core";
import type { DeviceLists, Settings, UiState } from "./types";

export const api = {
  setEqualizer: (target: string, config: import("./types").EqConfig) => invoke("set_equalizer", { target, config }),
  getDevices: () => invoke<DeviceLists>("get_devices"),
  getUiState: () => invoke<UiState>("get_ui_state"),

  setInputDevice: (name: string | null) => invoke("set_input_device", { name }),
  setMonitorDevice: (name: string | null) => invoke("set_monitor_device", { name }),
  setVirtualOutput: (name: string | null) => invoke("set_virtual_output", { name }),

  setMode: (enhanced: boolean) => invoke("set_mode", { enhanced }),
  setNoiseStrength: (value: number) => invoke("set_noise_strength", { value }),
  setStudio: (on: boolean) => invoke("set_studio", { on }),
  setPreset: (name: string) => invoke("set_preset", { name }),
  setMonitorEnabled: (on: boolean) => invoke("set_monitor_enabled", { on }),
  setMonitorVolume: (value: number) => invoke("set_monitor_volume", { value }),
  setAutoGain: (on: boolean) => invoke("set_auto_gain", { on }),

  setSetting: (key: string, value: unknown) => invoke("set_setting", { key, value }),
  virtualMicStatus: () =>
    invoke<{ status: string; backend: string | null; selectInApps: string | null; message: string }>(
      "virtual_mic_status"
    ),
  virtualMicSetup: () => invoke("virtual_mic_setup"),
};

export function defaultName(devs: { name: string; isDefault: boolean }[]): string | null {
  return devs.find((d) => d.isDefault)?.name ?? null;
}
