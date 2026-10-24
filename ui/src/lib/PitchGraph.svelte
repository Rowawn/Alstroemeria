<script lang="ts">
  import { highLow, morae } from "./pitch";

  let { reading, downstep }: { reading: string; downstep: number } = $props();

  const m = $derived(morae(reading));
  const levels = $derived(highLow(m.length, downstep));
  const W = 22;
  const x = (i: number) => 8 + i * W;
  const y = (high: boolean) => (high ? 8 : 24);
  const points = $derived(levels.map((h, i) => `${x(i)},${y(h)}`).join(" "));
</script>

<svg width={x(levels.length - 1) + 14} height="52" role="img" aria-label={`Pitch accent ${downstep}`}>
  <polyline {points} fill="none" stroke="var(--accent)" stroke-width="2" />
  {#each levels as h, i}
    <circle cx={x(i)} cy={y(h)} r={i === levels.length - 1 ? 3 : 4} fill={i === levels.length - 1 ? "none" : "var(--accent)"} stroke="var(--accent)" />
  {/each}
  {#each m as ch, i}
    <text x={x(i)} y="46" text-anchor="middle" font-size="13" fill="currentColor">{ch}</text>
  {/each}
</svg>
