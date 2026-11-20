import { useEffect, useRef, useState, type CSSProperties } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Toggle } from './components';
import './eye-tracker.css';

type TrackerState = {
  running: boolean; paused?: boolean; face?: boolean; status?: string; error?: string;
  previewFps?: number; fps?: number; cameraFps?: number; latency?: number; confidence?: number;
  calibrated?: boolean; calibrating?: boolean; calibration?: string; cameraInfo?: string;
  pose?: number[]; settings?: Record<string, number | string | boolean>;
};

function CameraPreview({running}: {running: boolean}) {
  const image = useRef<HTMLImageElement>(null);
  const [visible, setVisible] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    setVisible(false);
    setError('');
    if (!running) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let animation = 0;
    let latest: {image: string; sentAt: number} | undefined;
    (async () => {
      const stop = await listen<{image: string; sentAt: number}>('eye-tracker-preview', event => {
        if (disposed) return;
        latest = event.payload;
        if (!animation) animation = requestAnimationFrame(() => {
          animation = 0;
          if (!disposed && image.current && latest && Date.now() - latest.sentAt < 400) {
            image.current.src = latest.image;
          }
          latest = undefined;
        });
      });
      if (disposed) { stop(); return; }
      unlisten = stop;
      await invoke('eye_tracker_command', {command: {action: 'preview', enabled: true}});
    })().catch(e => {if (!disposed) setError(String(e));});
    return () => {
      disposed = true;
      cancelAnimationFrame(animation);
      if (unlisten) {
        unlisten();
        invoke('eye_tracker_command', {command: {action: 'preview', enabled: false}}).catch(()=>{});
      }
    };
  }, [running]);
  return <div className="eye-preview">
    <img ref={image} alt="Live webcam tracking preview" style={{display: visible ? 'block' : 'none'}} onLoad={() => setVisible(true)}/>
    {!visible && <div><svg viewBox="0 0 48 48" width="48" height="48" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="M4 24s7-13 20-13 20 13 20 13-7 13-20 13S4 24 4 24Z"/><circle cx="24" cy="24" r="7"/></svg><p>{error || (running ? 'Connecting to your camera…' : 'Start tracking to open your camera')}</p></div>}
  </div>;
}

export function EyeTracker() {
  const [state, setState] = useState<TrackerState>({ running: false });
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try { const next = await invoke<TrackerState>('eye_tracker_status'); if (!disposed) setState(next); }
      catch (e) { if (!disposed) setError(String(e)); }
      if (!disposed) timer = setTimeout(poll, 250);
    };
    poll();
    return () => { disposed = true; clearTimeout(timer); };
  }, []);
  const action = async (name: string) => {
    setError(''); setBusy(true);
    try {
      if (name === 'start' || name === 'stop') await invoke(`eye_tracker_${name}`);
      else await invoke('eye_tracker_command', { command: { action: name } });
      setState(await invoke<TrackerState>('eye_tracker_status'));
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  const setting = (key: string, value: string | number | boolean) => {
    setError('');
    invoke('eye_tracker_command', { command: { action: 'setting', key, value } }).catch(e => setError(String(e)));
  };
  const s = state.settings ?? {};
  const toggle = (key: string, label: string, hint: string) => <div className="row spaced" key={key}>
    <span className="row-label">{label}<span className="sub">{hint}</span></span>
    <Toggle on={Boolean(s[key])} onChange={value => setting(key, value)} />
  </div>;
  const range = (key: string, label: string, min: number, max: number, step: number) => <label className="eye-range" key={key}>
    <span>{label}<output>{Number(s[key] ?? min).toFixed(step < 1 ? 1 : 0)}</output></span>
    <input aria-label={label} style={{"--pct": `${(Number(s[key] ?? min)-min)/(max-min)*100}%`} as CSSProperties} className="slider" type="range" min={min} max={max} step={step} value={Number(s[key] ?? min)} onChange={e => setting(key, Number(e.target.value))}/>
  </label>;
  return <div className="eye-page">
    <section className="card eye-heading">
      <div><p className="eyebrow">VISION</p><h2>Eye & head tracking</h2><p className="hint">Your webcam, your movement. Tracking runs locally alongside your voice tools.</p></div>
      <div className="eye-actions"><span className={'pill '+(state.running && !state.paused ? 'running' : 'stopped')}>{state.running ? state.paused ? 'PAUSED' : 'TRACKING' : 'STOPPED'}</span>
        <button className="chip active" disabled={busy} onClick={() => action(state.running ? 'stop' : 'start')}>{busy ? 'Please wait…' : state.running ? 'Stop tracking' : 'Start tracking'}</button></div>
    </section>
    {(error || state.error) && <div role="alert" className="banner error">{error || state.error}</div>}
    <div className="eye-grid">
      <section className="card eye-camera"><div className="row"><h2>Camera preview</h2><span className={'dot '+(state.face && !state.paused ? 'ok' : 'dim')}/></div>
        <CameraPreview running={state.running}/>
        <p className="hint" role="status">{state.status || 'Camera is off.'}</p>
        <div className="eye-metrics">{[['Camera', `${state.cameraFps ?? 0} fps`], ['Preview', `${state.previewFps ?? 0} fps`], ['Tracking', `${state.fps ?? 0} fps`], ['Latency', `${state.latency ?? 0} ms`], ['Confidence', `${state.confidence ?? 0}%`]].map(([label,value])=><div key={label}><small>{label}</small><b>{value}</b></div>)}</div>
        {state.pose && <p className="eye-pose">Yaw {state.pose[3].toFixed(1)}° · Pitch {state.pose[4].toFixed(1)}° · Roll {state.pose[5].toFixed(1)}°</p>}
      </section>
      <section className="card"><p className="eyebrow">CONTROL</p><h2>Calibration & movement</h2><p className="hint">{state.calibration || 'Start tracking to load your calibration.'}</p>
        <div className="eye-actions eye-calibration"><button className="chip active" disabled={!state.running || busy || state.calibrating} onClick={()=>action('calibrate')}>{state.calibrating ? 'Calibrating…' : 'Calibrate eyes & head'}</button><button className="chip" disabled={!state.calibrated || !state.running || state.calibrating} onClick={()=>action('accuracy')}>Test accuracy</button></div>
        <p className="hint">Calibration opens full screen. Follow the targets; press Escape to cancel.</p>
        <div className="eye-actions"><button className="chip" disabled={!state.running} onClick={()=>action('recenter')}>Recenter · F8</button><button className="chip" disabled={!state.running} onClick={()=>action('pause')}>{state.paused ? 'Resume' : 'Pause'} · F9</button></div>
        <fieldset disabled={!state.running || !state.settings}>
          {toggle('overlay','Gaze overlay','A click-through marker follows your gaze.')}
          {toggle('mouse','Control mouse · F10','Move the pointer with your gaze; F10 switches this off.')}
          {range('stability','Gaze stability',5,90,1)}
          {range('gaze_smoothing','Pointer smoothing',0,1,0.1)}
          {range('smoothing','Head smoothing',0.1,5,0.1)}
        </fieldset>
      </section>
      <section className="card"><p className="eyebrow">CAPTURE</p><h2>Camera settings</h2><fieldset disabled={!state.running || !state.settings}>
        <label className="eye-select">Camera index<select value={Number(s.camera ?? 0)} onChange={e=>setting('camera', Number(e.target.value))}>{Array.from({length:8},(_,i)=><option key={i} value={i}>Camera {i}</option>)}</select></label>
        <label className="eye-select">Resolution<select value={String(s.resolution ?? '1920x1080')} onChange={e=>setting('resolution',e.target.value)}>{['640x480','1280x720','1920x1080','2560x1440'].map(r=><option key={r}>{r}</option>)}</select></label>
        {toggle('show_mesh','Face landmarks','Show tracked features in the preview.')}
        {toggle('low_light','Low-light boost','Improve local face contrast in dim rooms.')}
        {toggle('fast_exposure','Fast exposure','Use with bright lighting for higher camera frame rates.')}
      </fieldset><p className="hint">{state.cameraInfo || 'Camera details appear once tracking starts.'}</p></section>
      <section className="card"><p className="eyebrow">PLAY</p><h2>Game head tracking</h2><p className="hint">Send six-axis head movement to OpenTrack on this PC. Set its input to UDP and use the same port.</p><fieldset disabled={!state.running || !state.settings}>
        {toggle('udp','Send to OpenTrack','Local UDP output on 127.0.0.1.')}
        <label className="eye-select">UDP port<input aria-label="UDP port" type="number" min="1" max="65535" defaultValue={Number(s.udp_port ?? 4242)} key={String(s.udp_port ?? 4242)} onBlur={e=>setting('udp_port',Number(e.target.value))}/></label>
        {range('sens_yaw','Yaw sensitivity',0.1,3,0.1)}{range('sens_pitch','Pitch sensitivity',0.1,3,0.1)}{range('sens_roll','Roll sensitivity',0.1,3,0.1)}{range('sens_pos','Position sensitivity',0.1,3,0.1)}{range('deadzone','Head rotation deadzone',0,10,0.5)}
      </fieldset></section>
    </div>
  </div>;
}
