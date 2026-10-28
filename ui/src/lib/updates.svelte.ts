// Update checks (SPEC §17.4): desktop offers an in-app install, Android shows a banner linking to the release page.
import { invoke } from "@tauri-apps/api/core";

export type UpdateInfo = { version: string; autoCheck: boolean; canInstall: boolean };
export type Available = { version: string; notes: string; canInstall: boolean; pageUrl: string | null };

const DAY = 24 * 60 * 60 * 1000;

class UpdateState {
  info = $state<UpdateInfo | null>(null);
  available = $state<Available | null>(null);
  checking = $state(false);
  installing = $state(false);
  /** Set after a manual check that found nothing new. */
  upToDate = $state(false);
  error = $state("");
  dismissed = $state(false);
  private timer: ReturnType<typeof setInterval> | undefined;

  async start() {
    if (this.info) return;
    this.info = await invoke<UpdateInfo>("update_info");
    if (this.info.autoCheck) {
      void this.check(false);
      this.timer = setInterval(() => this.info?.autoCheck && void this.check(false), DAY);
    }
  }

  /** `manual` shows errors and "up to date"; an automatic check stays quiet unless it finds something. */
  async check(manual: boolean) {
    this.checking = true;
    this.error = "";
    this.upToDate = false;
    try {
      this.available = await invoke<Available | null>("update_check");
      this.dismissed = false;
      this.upToDate = manual && !this.available;
    } catch (e) {
      if (manual) this.error = String(e);
    } finally {
      this.checking = false;
    }
  }

  async setAuto(on: boolean) {
    await invoke("update_set_auto", { on });
    if (this.info) this.info.autoCheck = on;
    if (on) void this.check(false);
  }

  /** Download, install and restart (desktop). */
  async install() {
    this.installing = true;
    this.error = "";
    try {
      await invoke("update_install");
    } catch (e) {
      this.error = String(e);
      this.installing = false;
    }
  }
}

export const updates = new UpdateState();
