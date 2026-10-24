// LAN sync state shared by the top bar, the Devices tab and the views that refresh on `sync://applied` (SPEC §9.7).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type MediaPolicy = { audio: boolean; video: boolean; images: boolean; maxMb: number };
export type SyncPeer = {
  siteId: string;
  name: string;
  fingerprint: string;
  online: boolean;
  syncing: boolean;
  lastSeen: number | null;
  addr: string | null;
  port: number | null;
  mediaPolicy: MediaPolicy;
};
export type Nearby = { siteId: string; name: string; fp: string; addrs: string[]; port: number; pairing: boolean };
export type SyncStatus = { state: "idle" | "syncing" | "error"; message: string | null; lastSyncedAt: number | null };
export type Overview = {
  deviceName: string;
  fingerprint: string;
  siteId: string;
  port: number;
  status: SyncStatus;
  assets: [number, number] | null;
  pairing: boolean;
  peers: SyncPeer[];
  nearby: Nearby[];
};
export type PairStart = { svg: string; text: string; expiresAt: number; addrs: string[]; port: number };
export type Paired = { siteId: string; name: string; publicKey: string };

export const syncApi = {
  overview: () => invoke<Overview>("sync_overview"),
  setDeviceName: (name: string) => invoke<void>("set_device_name", { name }),
  pairStart: () => invoke<PairStart>("pair_start"),
  pairStop: () => invoke<void>("pair_stop"),
  pairByCode: (siteId: string | null, address: string | null) => invoke<Paired>("pair_by_code", { siteId, address }),
  pairByQr: (text: string) => invoke<Paired>("pair_by_qr", { text }),
  pairAnswer: (accept: boolean) => invoke<void>("pair_answer", { accept }),
  syncNow: (siteId?: string) => invoke<void>("sync_now", { siteId: siteId ?? null }),
  unpair: (siteId: string) => invoke<void>("unpair", { siteId }),
  setMediaPolicy: (siteId: string, policy: MediaPolicy) => invoke<void>("set_media_policy", { siteId, policy }),
  setPeerAddr: (siteId: string, address: string) => invoke<void>("set_peer_addr", { siteId, address }),
  downloadMedia: (hash: string) => invoke<boolean>("download_media", { hash }),
};

/** Reactive sync state. `tick` increments whenever synced data changed here, so views reload with `$effect`. */
class SyncState {
  overview = $state<Overview | null>(null);
  available = $state(true);
  tick = $state(0);
  /// Set while the two devices compare codes.
  code = $state<{ peerName: string; code: string } | null>(null);
  /// The last pairing result seen by the pairing-mode device.
  paired = $state<Paired | null>(null);
  private started = false;

  async refresh() {
    try {
      this.overview = await syncApi.overview();
      this.available = true;
    } catch {
      this.available = false;
    }
  }

  start() {
    if (this.started) return;
    this.started = true;
    void this.refresh();
    void listen<Overview>("sync://state", (e) => {
      if (e.payload && "deviceName" in e.payload) this.overview = e.payload;
    });
    void listen("sync://applied", () => this.tick++);
    void listen<{ peerName: string; code: string }>("sync://pair-code", (e) => (this.code = e.payload));
    void listen<Paired | { Ok: Paired } | { Err: string }>("sync://paired", (e) => {
      this.code = null;
      const p = e.payload as any;
      this.paired = p && "siteId" in p ? p : p?.Ok ?? null;
      void this.refresh();
    });
  }
}

export const sync = new SyncState();
