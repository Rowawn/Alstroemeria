import { extHost } from "./index";

/** Bumped whenever an extension starts, stops or errors, so components can re-read `extHost.status(id)`. */
export const extState = $state({ version: 0 });

extHost.subscribe(() => {
  extState.version++;
});
