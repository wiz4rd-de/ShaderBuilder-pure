// The right-region tabbed panel container (#48). Hosts the node inspector (#47)
// alongside the engine-driving panels (pass settings, viewport, source) in a
// simple tab strip; the preview pane sits below, always visible, since it is
// what the panels drive. This OWNS the right-region layout the App shell used to
// inline — App now just renders <PanelLayout/>.
import { useState } from "react";

import { ProblemsPanel } from "../compile/ProblemsPanel";
import { InspectorPanel } from "../inspector/InspectorPanel";
import { ParameterPanel } from "../parameters/ParameterPanel";
import { useDocumentStore } from "../store/documentStore";
import { PassSettingsPanel } from "./PassSettingsPanel";
import { SourcePanel } from "./SourcePanel";
import { ViewportPanel } from "./ViewportPanel";

type TabId = "inspector" | "params" | "pass" | "viewport" | "source" | "problems";

const TABS: { id: TabId; label: string }[] = [
  { id: "inspector", label: "Inspector" },
  { id: "params", label: "Parameters" },
  { id: "pass", label: "Pass" },
  { id: "viewport", label: "Viewport" },
  { id: "source", label: "Source" },
  { id: "problems", label: "Problems" },
];

export function PanelLayout(): React.JSX.Element {
  const [active, setActive] = useState<TabId>("inspector");
  // A live problem count badges the Problems tab so issues are visible without
  // opening it (the live compile loop, #54, keeps `problems` current).
  const problemCount = useDocumentStore((s) => s.problems.length);

  return (
    <section className="panels" aria-label="Panels">
      <div className="panels__tabs" role="tablist" aria-label="Panel tabs">
        {TABS.map((tab) => (
          <button
            key={tab.id}
            type="button"
            role="tab"
            aria-selected={active === tab.id}
            className={
              "panels__tab" + (active === tab.id ? " panels__tab--active" : "")
            }
            onClick={() => setActive(tab.id)}
          >
            {tab.label}
            {tab.id === "problems" && problemCount > 0 ? (
              <span className="panels__tab-badge" aria-label={`${problemCount} problems`}>
                {problemCount}
              </span>
            ) : null}
          </button>
        ))}
      </div>

      <div className="panels__content">
        {active === "inspector" && <InspectorPanel />}
        {active === "params" && <ParameterPanel />}
        {active === "pass" && <PassSettingsPanel />}
        {active === "viewport" && <ViewportPanel />}
        {active === "source" && <SourcePanel />}
        {active === "problems" && <ProblemsPanel />}
      </div>
    </section>
  );
}
