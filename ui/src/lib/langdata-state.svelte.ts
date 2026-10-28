// Language data (SPEC §17.3): installed state, download progress and the actions shared by the reader prompt
// and Settings → Languages.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type LangDataStatus = {
  id: string;
  label: string;
  version: string;
  size: number;
  installed: boolean;
  published: boolean;
};

/** The core reports missing data as `langdata:<id>`. */
export const needsLangData = (error: unknown): string | null => {
  const m = /^langdata:([\w-]+)$/.exec(String(error).trim());
  return m ? m[1] : null;
};

/** Error text for places that can't show the download card: names the Languages tab. */
export const errorText = (error: unknown): string => {
  const id = needsLangData(error);
  return id ? "This language needs its language data. Open the Languages tab to download it." : String(error);
};

export const megabytes = (bytes: number) => (bytes >= 1 << 20 ? `${Math.round(bytes / (1 << 20))} MB` : "a few MB");

const CHUNK = 1 << 20;

class LangDataState {
  packages = $state<LangDataStatus[]>([]);
  /** Download progress per id (0..1); present while downloading. */
  progress = $state<Record<string, number>>({});
  errors = $state<Record<string, string>>({});
  /** Increments when a package finished installing, so views can retry what failed without it. */
  tick = $state(0);
  private started = false;

  start() {
    if (this.started) return;
    this.started = true;
    void this.refresh();
    void listen<{ id: string; done: number; total: number }>("langdata://progress", (e) => {
      const { id, done, total } = e.payload;
      this.progress[id] = total ? done / total : 0;
    });
    void listen("langdata://changed", () => void this.refresh(true));
  }

  async refresh(changed = false) {
    this.packages = await invoke<LangDataStatus[]>("langdata_status");
    if (changed) this.tick += 1;
  }

  get(id: string) {
    return this.packages.find((p) => p.id === id);
  }

  async download(id: string) {
    this.errors[id] = "";
    this.progress[id] = 0;
    try {
      await invoke("langdata_download", { id });
    } catch (e) {
      this.errors[id] = String(e);
    } finally {
      delete this.progress[id];
      await this.refresh(true);
    }
  }

  cancel(id: string) {
    void invoke("langdata_cancel", { id });
  }

  /** "Import from file": upload the zip in chunks, then have the core verify and install it. */
  async importFile(file: File, forId?: string) {
    const key = forId ?? "";
    this.errors[key] = "";
    try {
      const upload = await invoke<string>("media_upload_begin");
      for (let off = 0; off < file.size; off += CHUNK) {
        const chunk = new Uint8Array(await file.slice(off, off + CHUNK).arrayBuffer());
        await invoke("media_upload_chunk", chunk, { headers: { "x-upload-id": upload } });
      }
      await invoke<string>("langdata_import", { uploadId: upload });
    } catch (e) {
      this.errors[key] = String(e);
    } finally {
      await this.refresh(true);
    }
  }
}

export const langdata = new LangDataState();
