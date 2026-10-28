// Bridge to the Android shell (`AlstAndroid`, added in MainActivity). Absent everywhere else.
export type LocalNetwork = "granted" | "denied" | "unneeded";

type Bridge = { localNetwork(): string; requestLocalNetwork(): void; openAppSettings(): void };

const bridge = (): Bridge | undefined => (globalThis as { AlstAndroid?: Bridge }).AlstAndroid;

/** Android 17 blocks LAN connections and mDNS without this permission; other systems don't have it. */
export const localNetworkState = (): LocalNetwork => {
  const s = bridge()?.localNetwork();
  return s === "granted" || s === "denied" ? s : "unneeded";
};
export const requestLocalNetwork = () => bridge()?.requestLocalNetwork();
export const openAppSettings = () => bridge()?.openAppSettings();
