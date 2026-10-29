// State of the pack review screen (SPEC §10.3): which items are checked, per-item progress, failures and retry.
import type { PackPlan, PackPlanItem, PackProgress, PackReport } from "./api";

export type ItemUi = "queued" | "running" | "done" | "skipped" | "failed";

/** Statuses the user may tick. An item that is already imported or needs a desktop is shown but not selectable. */
export function selectable(i: PackPlanItem): boolean {
  return i.status === "ready" || i.status === "license-warning";
}

export function statusLabel(i: PackPlanItem): string {
  switch (i.status) {
    case "needs-desktop":
      return "Import on a desktop; it will sync here";
    case "already-imported":
      return "Already imported";
    case "license-warning":
      return `Licence ${i.license ?? "unknown"} is not on the list for embedded text`;
    default:
      return "";
  }
}

export class Review {
  plan: PackPlan;
  checked = $state<Record<string, boolean>>({});
  seedVocab = $state(true);
  phase = $state<"review" | "running" | "done">("review");
  itemState = $state<Record<string, ItemUi>>({});
  errors = $state<Record<string, string>>({});
  position = $state({ index: 0, total: 0 });
  report = $state<PackReport | null>(null);

  constructor(plan: PackPlan) {
    this.plan = plan;
    for (const i of plan.items) this.checked[i.id] = i.checked && selectable(i);
  }

  toggle(id: string) {
    const item = this.plan.items.find((i) => i.id === id);
    if (item && selectable(item) && this.phase === "review") this.checked[id] = !this.checked[id];
  }

  /** Check every item that can be imported without a warning, or clear all. */
  setAll(on: boolean) {
    for (const i of this.plan.items) this.checked[i.id] = on && i.status === "ready";
  }

  get chosen(): string[] {
    return this.plan.items.filter((i) => this.checked[i.id]).map((i) => i.id);
  }

  get canImport(): boolean {
    return this.phase === "review" && this.plan.languageInstalled && (this.chosen.length > 0 || (this.seedVocab && this.plan.vocabTotal > 0));
  }

  get failedIds(): string[] {
    return this.plan.items.filter((i) => this.itemState[i.id] === "failed").map((i) => i.id);
  }

  /** Mark the chosen items as queued and move to the running phase. */
  start(ids: string[]) {
    this.phase = "running";
    this.report = null;
    for (const id of ids) {
      this.itemState[id] = "queued";
      delete this.errors[id];
    }
    this.position = { index: 0, total: ids.length };
  }

  onProgress(p: PackProgress) {
    this.position = { index: p.index, total: p.total };
    this.itemState[p.itemId] = p.state === "started" ? "running" : p.state;
    if (p.error) this.errors[p.itemId] = p.error;
  }

  finish(report: PackReport) {
    this.report = report;
    this.phase = "done";
    // Items never reached (cancelled) go back to unchecked-queued: they are simply not imported.
    for (const [id, st] of Object.entries(this.itemState)) if (st === "queued" || st === "running") delete this.itemState[id];
    for (const f of report.failed) {
      this.itemState[f.itemId] = "failed";
      this.errors[f.itemId] = f.error;
    }
  }

  /** Back to the review state for the failed items only (their statuses stay "ready"). */
  prepareRetry(): string[] {
    const ids = this.failedIds;
    this.phase = "review";
    for (const i of this.plan.items) this.checked[i.id] = ids.includes(i.id);
    return ids;
  }

  /** New plan from the core after an import: statuses changed, so selections restart. */
  replacePlan(plan: PackPlan) {
    this.plan = plan;
  }
}
