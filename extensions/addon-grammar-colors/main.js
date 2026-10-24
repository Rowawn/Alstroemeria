// Grammar colours (SPEC §16.10): colours words by the universal role in `<lang>.pos` (SPEC §16.4).

export async function decorate({ language, units, tokens }) {
  const settings = await alst.settings.get();
  const out = {};
  units.forEach((u, i) => {
    if (!u.isWord) return;
    const role = tokens[u.tokenRange[0]]?.ann?.[`${language}.pos`];
    if (typeof role === "string" && settings[role]) out[i] = [{ kind: "color", role }];
  });
  return out;
}
