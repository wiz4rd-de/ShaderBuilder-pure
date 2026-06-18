// The right-region tabbed panel container (#48). Hosts the node inspector (#47)
// alongside the engine-driving panels (pass settings, viewport, source) in a
// simple tab strip; the preview pane sits below, always visible, since it is
// what the panels drive. This OWNS the right-region layout the App shell used to
// inline — App now just renders <PanelLayout/>.
import { useState } from "react";

import { InspectorPanel } from "../inspector/InspectorPanel";
import { PassSettingsPanel } from "./PassSettingsPanel";
import { SourcePanel } from "./SourcePanel";
import { ViewportPanel } from "./ViewportPanel";

type TabId = "inspector" | "pass" | "viewport" | "source";

const TABS: { id: TabId; label: string }[] = [
  { id: "inspector", label: "Inspector" },
  { id: "pass", label: "Pass" },
  { id: "viewport", label: "Viewport" },
  { id: "source", label: "Source" },
];

export function PanelLayout(): React.JSX.Element {
  const [active, setActive] = useState<TabId>("inspector");

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
          </button>
        ))}
      </div>

      <div className="panels__content">
        {active === "inspector" && <InspectorPanel />}
        {active === "pass" && <PassSettingsPanel />}
        {active === "viewport" && <ViewportPanel />}
        {active === "source" && <SourcePanel />}
      </div>
    </section>
  );
}
