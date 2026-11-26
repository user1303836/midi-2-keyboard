// The profiles under /examples, bundled as text so they can be imported with one click.

import { api, useApp } from "./store";

export interface Example {
  file: string;
  title: string;
  detail: string;
  text: string;
}

export const EXAMPLES: Example[] = Object.entries(
  import.meta.glob("/examples/*.json", { eager: true, query: "?raw", import: "default" }) as Record<string, string>,
)
  .map(([path, text]) => {
    const file = path.split("/").pop()!.replace(/\.json$/, "");
    let names: string[] = [];
    let count = 0;
    try {
      const data = JSON.parse(text) as { profiles?: { name?: string; mappings?: unknown[] }[] };
      names = (data.profiles ?? []).map((p) => p.name ?? file);
      count = (data.profiles ?? []).reduce((sum, p) => sum + (p.mappings?.length ?? 0), 0);
    } catch {
      // A broken example simply shows its file name; importing it reports the problem.
    }
    return {
      file,
      title: names[0] ?? file,
      detail: `${names.length > 1 ? `${names.length} profiles · ` : ""}${count} mappings`,
      text,
    };
  })
  .sort((a, b) => a.title.localeCompare(b.title));

/** Adds an example's profiles to the config (through the backend's loader, so they are complete). */
export async function importExample(example: Example): Promise<void> {
  const { importProfiles, toast } = useApp.getState();
  try {
    const backend = await api();
    const parsed = await backend.parseProfiles(example.text);
    importProfiles(parsed);
    toast(
      `Added ${parsed.length === 1 ? `“${parsed[0].name}”` : `${parsed.length} example profiles`}. Open a mapping to see how it is built.`,
      { kind: "success" },
    );
  } catch (error) {
    toast(`Could not load the example: ${String(error)}`, { kind: "error" });
  }
}
