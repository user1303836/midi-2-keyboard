import { AlertTriangle, Cable, ShieldAlert } from "lucide-react";
import type { ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { deviceLabel } from "@/lib/describe";
import { api, useApp } from "@/lib/store";
import { cn } from "@/lib/utils";

/** Makes sure a message ends like a sentence. */
function sentence(text: string): string {
  const trimmed = text.trim();
  return /[.!?…]$/.test(trimmed) ? trimmed : `${trimmed}.`;
}

function Notice({
  tone,
  icon,
  children,
  actions,
}: {
  tone: "warn" | "danger" | "info";
  icon: ReactNode;
  children: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div
      role="status"
      className={cn(
        "flex items-center gap-2.5 border-b px-4 py-2",
        tone === "danger" && "bg-danger-soft text-danger",
        tone === "warn" && "bg-warn-soft text-warn",
        tone === "info" && "bg-raised text-fg-muted",
      )}
    >
      <span className="shrink-0">{icon}</span>
      <div className="selectable min-w-0 flex-1 leading-snug">{children}</div>
      <div className="flex shrink-0 items-center gap-1.5">{actions}</div>
    </div>
  );
}

/** Things that would otherwise make the app look broken, said plainly and with a way out. */
export function Notices() {
  const backendError = useApp((state) => state.backendError);
  const permissions = useApp((state) => state.permissions);
  const ports = useApp((state) => state.ports);
  const config = useApp((state) => state.config);
  const setPanel = useApp((state) => state.setPanel);
  const openVirtual = useApp((state) => state.openVirtualController);
  const monitorOpen = useApp((state) => state.monitorOpen);

  const failed = ports.filter((port) => port.state === "error");
  const connected = ports.filter((port) => port.state === "connected" && !port.is_virtual);
  const blocked = backendError !== "" || !permissions.can_inject;

  return (
    <>
      {blocked && (
        <Notice
          tone="danger"
          icon={<ShieldAlert size={16} />}
          actions={
            <>
              <Button size="sm" onClick={async () => (await api()).requestPermissions()}>
                Grant access
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setPanel("settings")}>
                Details
              </Button>
            </>
          }
        >
          <span className="font-semibold">The system is not letting this app send keys yet.</span>{" "}
          {backendError || permissions.hint}
        </Notice>
      )}
      {failed.length > 0 && (
        <Notice
          tone="warn"
          icon={<AlertTriangle size={16} />}
          actions={
            <Button size="sm" onClick={() => setPanel("devices")}>
              Details
            </Button>
          }
        >
          <span className="font-semibold">
            {failed.map((port) => deviceLabel(port.name, config)).join(", ")} could not be opened.
          </span>{" "}
          {sentence(failed[0].error || "Another program may be using it")} It is retried automatically.
        </Notice>
      )}
      {connected.length === 0 && failed.length === 0 && !monitorOpen && (
        <Notice
          tone="info"
          icon={<Cable size={16} />}
          actions={
            <Button size="sm" onClick={openVirtual}>
              Use the virtual controller
            </Button>
          }
        >
          No MIDI device is connected. Plug one in — it is picked up automatically — or try things
          out with the on-screen controller.
        </Notice>
      )}
    </>
  );
}
