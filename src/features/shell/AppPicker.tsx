import { AppWindow, Loader2 } from "lucide-react";
import { useState, type ReactNode } from "react";

import type { ForegroundApp } from "@/bindings/ForegroundApp";
import { Menu, MenuContent, MenuItem, MenuLabel, MenuTrigger } from "@/components/ui/overlays";
import { api } from "@/lib/store";

/**
 * A menu of the applications that are open right now, so nobody has to know (or type) that
 * Photoshop's process is called "Photoshop.exe".
 */
export function AppPicker({
  onPick,
  children,
}: {
  onPick: (app: ForegroundApp) => void;
  /** The trigger (a button). */
  children: ReactNode;
}) {
  const [apps, setApps] = useState<ForegroundApp[] | null>(null);

  const load = async (open: boolean) => {
    if (!open) return;
    setApps(null);
    try {
      const backend = await api();
      const list = await backend.runningApps();
      // Mapping this app to itself makes no sense.
      setApps(list.filter((app) => !/^midi-2-keyboard(\.exe)?$/i.test(app.process)));
    } catch {
      setApps([]);
    }
  };

  return (
    <Menu onOpenChange={load}>
      <MenuTrigger asChild>{children}</MenuTrigger>
      <MenuContent align="start" className="max-h-[320px] w-[340px] overflow-y-auto">
        <MenuLabel>Open applications</MenuLabel>
        {apps === null && (
          <div className="flex items-center gap-2 px-2 py-2 text-fg-faint">
            <Loader2 size={14} className="animate-spin" /> Looking…
          </div>
        )}
        {apps?.length === 0 && (
          <div className="px-2 py-2 text-sm leading-snug text-fg-faint">
            No applications could be listed on this system. Type the process name instead.
          </div>
        )}
        {apps?.map((app) => (
          <MenuItem
            key={`${app.process}-${app.pid}`}
            icon={<AppWindow size={14} />}
            className="h-auto py-1"
            onSelect={() => onPick(app)}
          >
            <div className="min-w-0">
              <div className="truncate font-medium">{app.process || app.app_id}</div>
              {app.title && <div className="truncate text-sm text-fg-faint">{app.title}</div>}
            </div>
          </MenuItem>
        ))}
      </MenuContent>
    </Menu>
  );
}
