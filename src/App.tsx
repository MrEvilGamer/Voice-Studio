import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { NavIcon, WindowGlyph } from "./Brand";
import { EyeTracker } from "./EyeTracker";
import { Equalizer } from "./Equalizer";
import { listen } from "@tauri-apps/api/event";
import { api, defaultName } from "./api";
import type { DeviceLists, DeviceSelection, EngineState, Settings, Snapshot } from "./types";
import { DeviceSelect, Meter, Modal, Segmented, Slider, Toggle } from "./components";

const STATE_LABEL: Record<EngineState, string> = {
  stopped: "STOPPED",
  starting: "STARTING",
  running: "RUNNING",
  restarting: "RESTARTING",
  error: "ERROR",
};

function baseDeviceName(name: string | null): string {
  return (name ?? "").replace(/\(.*?\)/g, "").trim().toLowerCase();
}

export default function App() {
  const [devices, setDevices] = useState<DeviceLists>({ inputs: [], outputs: [] });
  const [settings, setSettings] = useState<Settings | null>(null);
  const [selection, setSelection] = useState<DeviceSelection | null>(null);
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [page, setPage] = useState("studio");
  const [error, setError] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [showSetup, setShowSetup] = useState(false);
  const [vm, setVm] = useState<{
    status: string;
    backend: string | null;
    selectInApps: string | null;
    message: string;
  } | null>(null);
  const vmReady = vm?.status === "ready";
  const vmInstalling = vm?.status === "installing";
  const vmSelectInApps = vm?.selectInApps ?? null;

  const snapRef = useRef<Snapshot | null>(null);
  const inBar = useRef<HTMLDivElement>(null);
  const outBar = useRef<HTMLDivElement>(null);
  const noiseBar = useRef<HTMLDivElement>(null);
  const procText = useRef<HTMLSpanElement>(null);
  const pipeText = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    const handle = (event: PromiseRejectionEvent) => { event.preventDefault(); setError(String(event.reason)); api.getUiState().then(ui=>setSettings(ui.settings)).catch(()=>{}); };
    const context = (event: MouseEvent) => event.preventDefault();
    window.addEventListener("unhandledrejection", handle);
    document.addEventListener("contextmenu", context);
    return () => { window.removeEventListener("unhandledrejection", handle); document.removeEventListener("contextmenu", context); };
  }, []);

  // initial load + event subscriptions
  useEffect(() => {
    let disposed = false;
    (async () => {
      const ui = await api.getUiState();
      if (disposed) return;
      snapRef.current = ui.snapshot;
      setSnap(ui.snapshot);
      setSettings(ui.settings);
      setSelection(ui.selection);
      setDevices(ui.devices);
    })().catch(e => setError(String(e)));
    const un1 = listen<Snapshot>("engine-snapshot", (e) => {
      snapRef.current = e.payload;
      const p = e.payload;
      // meters/latency render at 60 fps via refs above; only re-render React
      // when a display-relevant field actually changes (state, warnings…)
      setSnap((old) => {
        if (
          old &&
          old.state === p.state &&
          old.clip === p.clip &&
          old.aiActive === p.aiActive &&
          old.dspActive === p.dspActive &&
          old.message === p.message
        ) {
          return old;
        }
        return p;
      });
    });
    const refreshVm = () =>
      api.virtualMicStatus().then((s) => setVm(s)).catch(() => {});
    refreshVm();
    const un2 = listen<DeviceLists>("devices-changed", (e) => {
      setDevices(e.payload);
      refreshVm();
    });
    const un3 = listen<{ stage: string; message: string }>("vm-setup", (e) => {
      setVm((old) => ({
        status:
          e.payload.stage === "done"
            ? "ready"
            : e.payload.stage === "error"
              ? "setup-required"
              : "installing",
        backend: old?.backend ?? null,
        selectInApps: old?.selectInApps ?? null,
        message: e.payload.message,
      }));
      if (e.payload.stage === "done") {
        api.getUiState().then((ui) => {
          setSettings(ui.settings);
          setSelection(ui.selection);
          setDevices(ui.devices);
        });
      }
    });
    return () => {
      disposed = true;
      un1.then((f) => f());
      un2.then((f) => f());
      un3.then((f) => f());
    };
  }, []);

  // 60 fps meter updates without React re-renders
  useEffect(() => {
    let raf = 0;
    const dbPct = (db: number) => {
      const c = Math.max(-60, Math.min(0, db));
      return ((c + 60) / 60) * 100;
    };
    const noisePct = (db: number) => Math.max(0, Math.min(100, (db / 28) * 100));
    const tick = () => {
      const s = snapRef.current;
      if (s) {
        if (inBar.current) inBar.current.style.width = dbPct(s.inputRmsDb) + "%";
        if (outBar.current) outBar.current.style.width = dbPct(s.outputRmsDb) + "%";
        if (noiseBar.current) noiseBar.current.style.width = noisePct(s.noiseRemovedDb) + "%";
        if (procText.current) procText.current.textContent = s.procMs.toFixed(1) + " ms";
        if (pipeText.current) pipeText.current.textContent = "≈ " + s.pipelineMs.toFixed(0) + " ms";
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, []);

  if (!settings) {
    return <div className="loading">{error ? `Could not start Voice Studio: ${error}` : "Starting Voice Studio…"}</div>;
  }

  const state = snap?.state ?? "starting";
  const defIn = defaultName(devices.inputs);
  const defOut = defaultName(devices.outputs);
  const inputActive = selection?.input ?? null;
  const monitorActive = selection?.monitor ?? null;
  const virtualActive = selection?.virtualOut ?? null;

  const strength = settings.noiseStrength;
  const strengthBand = strength === 0 ? "off" : strength <= 33 ? "low" : strength <= 66 ? "med" : "high";

  // feedback risk: input and monitor share the same physical device
  const feedbackRisk =
    settings.monitorEnabled &&
    inputActive !== null &&
    baseDeviceName(inputActive) !== "" &&
    baseDeviceName(inputActive) === baseDeviceName(monitorActive ?? defOut);

  const hintVirtual = virtualActive
    ? virtualActive.toLowerCase().includes("cable")
      ? virtualActive.replace(/input/i, "Output")
      : "the matching recording device of this playback endpoint"
    : null;

  const update = (patch: Partial<Settings>) => setSettings((s) => (s ? { ...s, ...patch } : s));

  return (
    <div className="desktop" onContextMenu={e => e.preventDefault()}>
      <div className="titlebar"><div className="titlebar-drag" data-tauri-drag-region onMouseDown={e=>{if(e.button===0)getCurrentWindow().startDragging().catch(err=>setError(String(err)));}}>VOICE STUDIO</div><div className="window-controls">{["minimize","maximize","close"].map(action=><button key={action} aria-label={action} className={action} onClick={()=>{const w=getCurrentWindow();(action==="minimize"?w.minimize():action==="maximize"?w.toggleMaximize():w.close()).catch(err=>setError(String(err)));}}><WindowGlyph kind={action}/></button>)}</div></div>
      <aside className="sidebar" aria-label="Workspace navigation">
        <div className="rail-brand"><img src="/brand/voice-studio-logo.png" alt="Voice Studio"/><div className="rail-wordmark">VOICE STUDIO <small>0.1.0</small></div></div>
        <nav aria-label="Main navigation">
          <button className={page==="studio"?"selected":""} aria-label="Dashboard" aria-current={page==="studio"?"page":undefined} title="Dashboard" onClick={()=>setPage("studio")}><NavIcon kind="studio"/><span className="nav-label">Dashboard</span></button>
          <button className={page==="equalizer"?"selected":""} aria-label="Equalizers" aria-current={page==="equalizer"?"page":undefined} title="Equalizers" onClick={()=>setPage("equalizer")}><NavIcon kind="equalizer"/><span className="nav-label">Equalizers</span></button>
          <button className={page==="eyes"?"selected":""} aria-label="Eye Tracking" aria-current={page==="eyes"?"page":undefined} title="Eye Tracking" onClick={()=>setPage("eyes")}><NavIcon kind="eyes"/><span className="nav-label">Eye Tracking</span></button>
          <button aria-label="Settings" title="Settings" onClick={()=>setShowSettings(true)}><NavIcon kind="settings"/><span className="nav-label">Settings</span></button>
        </nav>
        <div className="rail-local" title="Audio processing runs on this computer"><span className="dot ok"/><span className="nav-label">Local processing</span></div>
      </aside>
      <main className={"app page-"+page}>
      <header className="header">
        <div className="brand">
          <h1>{page==="studio"?"Dashboard":page==="eyes"?"Eye Tracking":"Equalizers"}</h1>
        </div>
        <div className="header-right">
          <span className="local-badge"><i/> LOCAL AUDIO</span>
          <span className={"pill " + state}>{STATE_LABEL[state]}</span>
          <button className="icon-btn" onClick={() => setShowSettings(true)} aria-label="Settings">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <circle cx="12" cy="12" r="3" />
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h.01a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51h.01a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v.01a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
            </svg>
          </button>
          <div className="top-identity"><div><b>Voice Studio</b><small>YOUR SOUND. YOUR WAY.</small></div><img src="/brand/voice-studio-logo.png" alt=""/></div>
        </div>
      </header>
      <div className="page-scroll">

      {error && <div className="banner error" role="alert">{error}<button className="link" onClick={()=>setError(null)}>Dismiss</button></div>}
      {state === "error" && (
        <div className="banner error">
          {snap?.message ?? "Audio engine error"} — plug in a microphone or pick another device below.
        </div>
      )}
      {state !== "error" && snap?.message && <div className="banner warn">{snap.message}</div>}
      {snap?.clip && (
        <div className="banner error">
          MICROPHONE CLIPPING — reduce your Windows microphone gain. Software cannot repair clipped audio.
        </div>
      )}
      {feedbackRisk && <div className="banner warn">Headphones recommended for live monitoring — your monitoring output is being picked up by this microphone.</div>}

      <div className="studio-grid" hidden={page!=="studio"}>
      <section className="card">
        <p className="eyebrow">RECORDING</p>
        <div className="row">
          <span className="row-label">Microphone</span>
          <span className={"dot " + (state === "running" ? "ok" : state === "error" ? "err" : "dim")} />
        </div>
        <DeviceSelect
          devices={devices.inputs}
          value={settings.inputDevice}
          placeholder={"Windows default" + (defIn ? ` (${defIn})` : "")}
          onChange={(v) => {
            update({ inputDevice: v });
            api.setInputDevice(v);
          }}
        />

        <div className="row spaced">
          <span className="row-label">Noise Removal</span>
          <span className="row-value">{strength}%</span>
        </div>
        <Segmented
          options={[
            { label: "OFF", value: "off" },
            { label: "LOW", value: "low" },
            { label: "MED", value: "med" },
            { label: "HIGH", value: "high" },
          ]}
          value={strengthBand}
          onChange={(v) => {
            const val = v === "off" ? 0 : v === "low" ? 33 : v === "med" ? 66 : 100;
            update({ noiseStrength: val });
            api.setNoiseStrength(val);
          }}
        />
        <Slider
          value={strength}
          min={0}
          max={100}
          onChange={(v) => {
            update({ noiseStrength: v });
            api.setNoiseStrength(v);
          }}
        />

        <div className="row spaced">
          <span className="row-label">
            Studio Enhance
            <span className="sub">EQ · de-esser · compressor · limiter</span>
          </span>
          <Toggle
            on={settings.studioEnhance}
            onChange={(on) => {
              update({ studioEnhance: on });
              api.setStudio(on);
            }}
          />
        </div>

        <div className="presets">
          {["natural", "studio", "gaming", "broadcast", "podcast", "warm", "clarity", "meeting"].map((p) => (
            <button
              key={p}
              title={{natural:"Light shaping, keeps your original tone",studio:"Balanced presence and controlled dynamics",gaming:"Clear team communication",broadcast:"Full, consistent presenter voice",podcast:"Smooth spoken-word delivery",warm:"Softer highs and fuller low mids",clarity:"More articulation and less muddiness",meeting:"Speech focus for everyday calls"}[p]}
              className={"chip" + (settings.preset === p ? " active" : "")}
              onClick={() => {
                update({ preset: p });
                api.setPreset(p);
              }}
            >
              {p[0].toUpperCase() + p.slice(1)}
            </button>
          ))}
        </div>
      </section>

      <section className="card signal-panel">
        <p className="eyebrow">REAL-TIME SIGNAL</p><h2 className="panel-title">Levels & processing</h2>
        <div className="signal-surface">
        <Meter label="INPUT" barRef={inBar} accent="var(--accent)" />
        <Meter label="OUTPUT" barRef={outBar} accent="var(--accent2)" />
        <Meter label="NOISE REMOVED" barRef={noiseBar} accent="var(--ok)" />
        <div className="stats">
          <span>
            Processing: <b ref={procText}>—</b>
          </span>
          <span>
            Engine buffer: <b ref={pipeText}>—</b>
          </span>
          <span className={snap?.aiActive ? "ok" : "warn"}>
            {snap?.aiActive ? "AI active" : "AI fallback"}
          </span>
        </div>


      </div>
      <div className="mode-compare">
        <Segmented
          large
          options={[
            { label: "RAW", value: "raw" },
            { label: "ENHANCED", value: "enhanced" },
          ]}
          value={settings.modeEnhanced ? "enhanced" : "raw"}
          onChange={(v) => {
            const enhanced = v === "enhanced";
            update({ modeEnhanced: enhanced });
            api.setMode(enhanced);
          }}
        />
      </div>
      </section>

      <section className="card">
        <p className="eyebrow">PLAYBACK</p>
        <div className="row">
          <span className="row-label">Monitoring</span>
        </div>
        <div className="row spaced">
          <span className="row-label">
            Listen To Myself
            <span className="sub">hear the processed signal live</span>
          </span>
          <Toggle
            on={settings.monitorEnabled}
            onChange={(on) => {
              update({ monitorEnabled: on });
              api.setMonitorEnabled(on);
            }}
          />
        </div>
        <DeviceSelect
          devices={devices.outputs}
          value={settings.monitorDevice}
          placeholder={"Windows default" + (defOut ? ` (${defOut})` : "")}
          onChange={(v) => {
            update({ monitorDevice: v });
            api.setMonitorDevice(v);
          }}
        />
        <div className="row spaced">
          <span className="row-label">Volume</span>
          <span className="row-value">{settings.monitorVolume}%</span>
        </div>
        <Slider
          value={settings.monitorVolume}
          min={0}
          max={100}
          onChange={(v) => {
            update({ monitorVolume: v });
            api.setMonitorVolume(v);
          }}
        />
        <p className="hint">Headphones recommended for live monitoring.</p>
      </section>

      <section className="card routing-panel">
        <p className="eyebrow">CONNECT TO YOUR APPS</p>
        <div className="row">
          <span className="row-label">Virtual Microphone</span>
          <span
            className={"dot " + (vmReady ? "ok pulse" : vmInstalling ? "dim pulse" : "dim")}
          />
          <span className={"status-text " + (vmReady ? "ok" : "warn")}>
            {vmInstalling ? "SETTING UP…" : vmReady ? "READY" : "SETUP REQUIRED"}
          </span>
        </div>
        <p className="hint">
          {vm?.message ??
            "Feeds everything the app processes into a microphone other apps can select."}
        </p>
        {vmSelectInApps && vmReady && (
          <p className="hint">
            Select <b>{vmSelectInApps}</b> as your microphone in Discord / FiveM / OBS.
          </p>
        )}
        {!vmReady && !vmInstalling && (
          <button className="chip vm-button" onClick={() => api.virtualMicSetup()}>
            Automatic setup
          </button>
        )}
        <button className="link" onClick={() => setShowSetup((s) => !s)}>
          {showSetup ? "Hide details" : "How to use in Discord / FiveM / OBS"}
        </button>
        {showSetup && (
          <div className="setup">
            <p>Select the recording endpoint shown above in your calling or streaming app.</p>
            <p>
              During development, a temporary virtual device stands in for it. After setup, open
              Discord / FiveM / OBS and select the microphone this app tells you to use. Routing is
              automatic from then on.
            </p>
            <p className="dim">All processing is local. Your real microphone is never modified.</p>
          </div>
        )}
      </section>

      </div>
      {page==="eyes" && <EyeTracker/>}
      {page==="equalizer" && <div className="equalizer-page">{(["mic","speaker"] as const).map(target=><Equalizer key={target} target={target} config={target==="mic"?settings.micEq:settings.speakerEq} onChange={async config=>{update(target==="mic"?{micEq:config}:{speakerEq:config});await api.setEqualizer(target,config);}}/>)}</div>}
      <Modal open={showSettings} onClose={() => setShowSettings(false)}>
        <div className="setting-row">
          <span>Start with Windows</span>
          <Toggle
            on={settings.startWithWindows}
            onChange={(on) => {
              update({ startWithWindows: on });
              api.setSetting("startWithWindows", on);
            }}
          />
        </div>
        <div className="setting-row">
          <span>Close to tray</span>
          <Toggle
            on={settings.minimizeToTray}
            onChange={(on) => {
              update({ minimizeToTray: on });
              api.setSetting("minimizeToTray", on);
            }}
          />
        </div>
        <div className="setting-row">
          <span>Launch minimized</span>
          <Toggle
            on={settings.launchMinimized}
            onChange={(on) => {
              update({ launchMinimized: on });
              api.setSetting("launchMinimized", on);
            }}
          />
        </div>
        <div className="setting-row">
          <span>
            Auto input gain
            <span className="sub">slow level normalization</span>
          </span>
          <Toggle
            on={settings.autoGain}
            onChange={(on) => {
              update({ autoGain: on });
              api.setAutoGain(on);
            }}
          />
        </div>
        <div className="setting-row col">
          <span>Processing quality</span>
          <Segmented
            options={[
              { label: "Low latency", value: "low" },
              { label: "Balanced", value: "balanced" },
              { label: "Safe", value: "safe" },
            ]}
            value={settings.quality}
            onChange={(v) => {
              update({ quality: v });
              api.setSetting("quality", v);
            }}
          />
          <span className="sub">buffer size — restarts the audio engine when changed</span>
        </div>
        <p className="hint">All processing runs locally. No audio ever leaves this machine.</p>
      </Modal>
    </div></main></div>
  );
}
