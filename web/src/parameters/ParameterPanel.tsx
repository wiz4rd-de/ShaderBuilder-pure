// Parameter panel (#53) — surfaces every declared `#pragma parameter` (from Param
// nodes AND whole-pass `#pragma parameter` lines) as a LIVE slider that drives the
// engine's parameter UBO via `set_parameter` with NO recompile.
//
// State: the global declaration list comes from `useCollectedParameters`; the live
// knob positions are a value map keyed by NAME, reconciled on every declaration
// change (persisting names keep their value, vanished names drop, new names seed
// their default). Slider drags fire `set_parameter` THROTTLED per animation frame
// so a drag does not flood the IPC bridge; the engine clamps + ignores unknowns.
import { useEffect, useMemo, useRef, useState } from "react";

import type { Parameter } from "../bindings/Parameter";
import { useCollectedParameters } from "./useCollectedParameters";
import { reconcileValues, valueMapsEqual, type ValueMap } from "./reconcile";
import { createParameterSender } from "./setParameter";

/** A single parameter row: label + slider + numeric entry + reset-to-default. */
function ParameterRow({
  param,
  value,
  onChange,
}: {
  param: Parameter;
  value: number;
  onChange: (value: number) => void;
}): React.JSX.Element {
  const atDefault = value === param.default;
  // A bare numeric entry mirrors the slider but accepts out-of-step typing; we
  // clamp on the controlled value so the field can't show an impossible position.
  return (
    <div className="param-row" aria-label={`Parameter ${param.name}`}>
      <div className="param-row__head">
        <span className="param-row__label" title={param.name}>
          {param.label || param.name}
        </span>
        <button
          type="button"
          className="param-row__reset"
          aria-label={`Reset ${param.name} to default`}
          disabled={atDefault}
          onClick={() => onChange(param.default)}
          title={`Reset to default (${param.default})`}
        >
          ⟲
        </button>
      </div>
      <div className="param-row__controls">
        <input
          type="range"
          className="param-row__slider"
          aria-label={`${param.name} slider`}
          min={param.min}
          max={param.max}
          step={param.step || "any"}
          value={value}
          onChange={(e) => onChange(Number(e.target.value))}
        />
        <input
          type="number"
          className="panel__input param-row__num"
          aria-label={`${param.name} value`}
          min={param.min}
          max={param.max}
          step={param.step || "any"}
          value={value}
          onChange={(e) => {
            const v = Number(e.target.value);
            if (Number.isFinite(v)) {
              onChange(v);
            }
          }}
        />
      </div>
    </div>
  );
}

export function ParameterPanel(): React.JSX.Element {
  const params = useCollectedParameters();

  // One throttled IPC sender for the panel's lifetime; flushed on unmount.
  const senderRef = useRef(createParameterSender());
  useEffect(() => {
    const sender = senderRef.current;
    return () => sender.cancel();
  }, []);

  // Live knob positions keyed by parameter name. Initialised from defaults and
  // reconciled whenever the declaration set changes (persisting names keep value).
  const [values, setValues] = useState<ValueMap>(() => reconcileValues({}, params));

  // Reconcile on declaration changes WITHOUT clobbering live drag values. We only
  // update state when the reconciled map actually differs (avoids render loops).
  useEffect(() => {
    setValues((prev) => {
      const next = reconcileValues(prev, params);
      return valueMapsEqual(prev, next) ? prev : next;
    });
  }, [params]);

  const byName = useMemo(() => {
    const m = new Map<string, Parameter>();
    for (const p of params) {
      m.set(p.name, p);
    }
    return m;
  }, [params]);

  const onChange = (name: string, raw: number): void => {
    const param = byName.get(name);
    if (!param) {
      return;
    }
    const lo = Math.min(param.min, param.max);
    const hi = Math.max(param.min, param.max);
    const value = Math.min(hi, Math.max(lo, raw));
    setValues((prev) => (prev[name] === value ? prev : { ...prev, [name]: value }));
    senderRef.current.set(name, value);
  };

  if (params.length === 0) {
    return (
      <div className="panel__body" aria-label="Parameters">
        <div className="panel__placeholder">
          No parameters declared. Add a Parameter node or a whole-pass{" "}
          <code>#pragma parameter</code> to expose a live slider.
        </div>
      </div>
    );
  }

  return (
    <div className="panel__body" aria-label="Parameters">
      <div className="param-list">
        {params.map((param) => (
          <ParameterRow
            key={param.name}
            param={param}
            value={values[param.name] ?? param.default}
            onChange={(v) => onChange(param.name, v)}
          />
        ))}
      </div>
    </div>
  );
}
