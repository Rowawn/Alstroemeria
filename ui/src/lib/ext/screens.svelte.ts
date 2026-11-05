// UI state for extension screens (SPEC §11.1): the overlay opened by `alst.ui.openScreen` and the file prompt.

export const screens = $state<{
  /** A screen shown over whatever is open, with the payload it was opened with. */
  overlay: { extId: string; screenId: string; payload: unknown; nonce: number } | null;
  /** A pending request for a file from an extension. */
  filePrompt: { extId: string; accept: string; resolve: (v: { uploadId: string; name: string } | null) => void } | null;
}>({ overlay: null, filePrompt: null });

let nonce = 0;

/** A part the extension asked the reader to open; the page shell takes it and clears it. */
export const nav = $state<{ part: { partId: string; start: number } | null }>({ part: null });

export function openPart(partId: string, start: number) {
  if (partId) nav.part = { partId, start };
}

export function openScreen(extId: string, screenId: string, payload: unknown) {
  screens.overlay = { extId, screenId, payload, nonce: ++nonce };
}

export function closeScreen() {
  screens.overlay = null;
}

export function pickFile(extId: string, accept: string): Promise<{ uploadId: string; name: string } | null> {
  screens.filePrompt?.resolve(null);
  return new Promise((resolve) => {
    screens.filePrompt = { extId, accept, resolve };
  });
}
