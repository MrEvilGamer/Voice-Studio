import { ReactNode, RefObject } from "react";

export function DeviceSelect(props: {
  devices: { name: string; isDefault: boolean }[];
  value: string | null;
  placeholder: string;
  onChange: (v: string | null) => void;
}) {
  const { devices, value, placeholder, onChange } = props;
  return (
    <div className="select-wrap">
      <select
        value={value ?? ""}
        onChange={(e) => onChange(e.target.value === "" ? null : e.target.value)}
      >
        <option value="">{placeholder}</option>
        {devices.map((d) => (
          <option key={d.name} value={d.name}>
            {d.name}
            {d.isDefault ? "  ·default" : ""}
          </option>
        ))}
      </select>
    </div>
  );
}

export function Toggle(props: { on: boolean; onChange: (on: boolean) => void }) {
  return (
    <button
      className={"toggle" + (props.on ? " on" : "")}
      role="switch"
      aria-checked={props.on}
      onClick={() => props.onChange(!props.on)}
    >
      <span className="knob" />
    </button>
  );
}

export function Segmented(props: {
  options: { label: string; value: string }[];
  value: string;
  onChange: (v: string) => void;
  large?: boolean;
}) {
  return (
    <div className={"segmented" + (props.large ? " large" : "")}>
      {props.options.map((o) => (
        <button
          key={o.value}
          className={o.value === props.value ? "active" : ""}
          onClick={() => props.onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Slider(props: {
  value: number;
  min: number;
  max: number;
  onChange: (v: number) => void;
}) {
  const pct = ((props.value - props.min) / (props.max - props.min)) * 100;
  return (
    <input
      type="range"
      className="slider"
      min={props.min}
      max={props.max}
      value={props.value}
      style={{ ["--pct" as string]: pct + "%" }}
      onChange={(e) => props.onChange(Number(e.target.value))}
    />
  );
}

export function Meter(props: {
  label: string;
  barRef: RefObject<HTMLDivElement>;
  accent?: string;
  valueRef?: RefObject<HTMLSpanElement>;
}) {
  return (
    <div className="meter">
      <span className="meter-label">{props.label}</span>
      <div className="meter-track">
        <div className="meter-fill" ref={props.barRef} style={{ background: props.accent }} />
      </div>
      {props.valueRef && <span className="meter-value" ref={props.valueRef} />}
    </div>
  );
}

export function Modal(props: { open: boolean; onClose: () => void; children: ReactNode }) {
  if (!props.open) return null;
  return (
    <div className="modal-backdrop" onClick={props.onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <span>Settings</span>
          <button className="icon-btn" onClick={props.onClose} aria-label="Close">
            ✕
          </button>
        </div>
        {props.children}
      </div>
    </div>
  );
}
