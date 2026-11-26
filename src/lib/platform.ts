export type Platform = "windows" | "macos" | "linux";

function detect(): Platform {
  const ua = navigator.userAgent.toLowerCase();
  if (ua.includes("mac")) return "macos";
  if (ua.includes("win")) return "windows";
  return "linux";
}

let current: Platform = detect();

/** The platform the app is running on (corrected by the backend once it has answered). */
export function platform(): Platform {
  return current;
}

export function setPlatform(value: string): void {
  if (value === "windows" || value === "macos" || value === "linux") current = value;
}

/** `true` when running inside the Tauri shell (as opposed to a plain browser tab). */
export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
