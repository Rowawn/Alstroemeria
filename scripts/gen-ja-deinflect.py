#!/usr/bin/env python3
"""Generates extensions/lang-ja/deinflect.json (SPEC §16.5) from a small conjugation table.

The rules are original work derived from standard Japanese conjugation patterns; no third-party
rule files are used or consulted. Edit this script, not the JSON. Order matters: when two rules
produce the same (text, conditions), the one listed first (and so its reason) wins.
"""
import json, pathlib

# ending: (a, i, e, o, te, ta) rows of the godan conjugation for each dictionary-form ending
GODAN = {
    "う": ("わ", "い", "え", "お", "って", "った"),
    "く": ("か", "き", "け", "こ", "いて", "いた"),
    "ぐ": ("が", "ぎ", "げ", "ご", "いで", "いだ"),
    "す": ("さ", "し", "せ", "そ", "して", "した"),
    "つ": ("た", "ち", "て", "と", "って", "った"),
    "ぬ": ("な", "に", "ね", "の", "んで", "んだ"),
    "ぶ": ("ば", "び", "べ", "ぼ", "んで", "んだ"),
    "む": ("ま", "み", "め", "も", "んで", "んだ"),
    "る": ("ら", "り", "れ", "ろ", "って", "った"),
}

rules = []


def add(si, so, cin, cout, reason):
    rules.append({"suffixIn": si, "suffixOut": so, "condIn": cin, "condOut": cout, "reason": reason})


def godan(fn):
    """Call fn(u, a, i, e, o, te, ta) for every godan ending."""
    for u, (a, i, e, o, te, ta) in GODAN.items():
        fn(u, a, i, e, o, te, ta)


# ── compound chains first so they win dedupe against the generic rules ─────────────────
add("させられる", "させる", ["v1"], ["v1"], "passive")          # 食べさせられた: causative, passive, past
add("される", "する", ["v1"], ["vs"], "passive")
add("させる", "する", ["v1"], ["vs"], "causative")
add("こさせる", "くる", ["v1"], ["vk"], "causative")
add("来させる", "来る", ["v1"], ["vk"], "causative")
add("こられる", "くる", ["v1"], ["vk"], "passive-potential")
add("来られる", "来る", ["v1"], ["vk"], "passive-potential")

# ── terminal forms: past (た/だ) and te (て/で) ─────────────────────────────────────────
add("た", "る", ["past"], ["v1"], "past")
add("て", "る", ["te"], ["v1"], "te")
add("行った", "行く", ["past"], ["v5"], "past")
add("行って", "行く", ["te"], ["v5"], "te")
add("いった", "いく", ["past"], ["v5"], "past")
add("いって", "いく", ["te"], ["v5"], "te")
add("した", "する", ["past"], ["vs"], "past")
add("して", "する", ["te"], ["vs"], "te")
add("来た", "来る", ["past"], ["vk"], "past")
add("来て", "来る", ["te"], ["vk"], "te")
add("きた", "くる", ["past"], ["vk"], "past")
add("きて", "くる", ["te"], ["vk"], "te")
add("かった", "い", ["past"], ["adj-i"], "past")
add("くて", "い", ["te"], ["adj-i"], "te")
add("よかった", "いい", ["past"], ["adj-i"], "past")
add("よくて", "いい", ["te"], ["adj-i"], "te")


def f_past_te(u, a, i, e, o, te, ta):
    add(ta, u, ["past"], ["v5"], "past")
    add(te, u, ["te"], ["v5"], "te")


godan(f_past_te)

# ── tara-conditional (past + ら) ───────────────────────────────────────────────────────
add("たら", "る", ["past"], ["v1"], "tara-conditional")
add("したら", "する", ["past"], ["vs"], "tara-conditional")
add("来たら", "来る", ["past"], ["vk"], "tara-conditional")
add("きたら", "くる", ["past"], ["vk"], "tara-conditional")
add("かったら", "い", ["past"], ["adj-i"], "tara-conditional")
add("よかったら", "いい", ["past"], ["adj-i"], "tara-conditional")
godan(lambda u, a, i, e, o, te, ta: add(ta + "ら", u, ["past"], ["v5"], "tara-conditional"))

# ── negative (ない behaves like an i-adjective) ────────────────────────────────────────
add("ない", "る", ["adj-i"], ["v1"], "negative")
add("しない", "する", ["adj-i"], ["vs"], "negative")
add("こない", "くる", ["adj-i"], ["vk"], "negative")
add("来ない", "来る", ["adj-i"], ["vk"], "negative")
add("くない", "い", ["adj-i"], ["adj-i"], "negative")
add("よくない", "いい", ["adj-i"], ["adj-i"], "negative")
godan(lambda u, a, i, e, o, te, ta: add(a + "ない", u, ["adj-i"], ["v5"], "negative"))
add("なきゃ", "ない", [], ["adj-i"], "conditional (colloquial)")
add("なくちゃ", "ない", [], ["adj-i"], "conditional (colloquial)")

# ── ば-conditional ─────────────────────────────────────────────────────────────────────
add("れば", "る", ["ba"], ["v1"], "ba-conditional")
add("すれば", "する", ["ba"], ["vs"], "ba-conditional")
add("来れば", "来る", ["ba"], ["vk"], "ba-conditional")
add("くれば", "くる", ["ba"], ["vk"], "ba-conditional")
add("ければ", "い", ["ba"], ["adj-i"], "ba-conditional")
add("よければ", "いい", ["ba"], ["adj-i"], "ba-conditional")
godan(lambda u, a, i, e, o, te, ta: add(e + "ば", u, ["ba"], ["v5"], "ba-conditional"))

# ── polite ます: stem forms ────────────────────────────────────────────────────────────
add("ませんでした", "ません", ["past"], ["pneg"], "past")
add("ません", "ます", ["pneg"], ["masu"], "negative")
add("ました", "ます", ["past"], ["masu"], "past")
add("ましょう", "ます", ["volitional"], ["masu"], "volitional")
add("ます", "る", ["masu"], ["v1"], "polite")
add("します", "する", ["masu"], ["vs"], "polite")
add("きます", "くる", ["masu"], ["vk"], "polite")
add("来ます", "来る", ["masu"], ["vk"], "polite")
godan(lambda u, a, i, e, o, te, ta: add(i + "ます", u, ["masu"], ["v5"], "polite"))

# ── desire ─────────────────────────────────────────────────────────────────────────────
add("たい", "る", ["adj-i"], ["v1"], "desire")
add("したい", "する", ["adj-i"], ["vs"], "desire")
add("きたい", "くる", ["adj-i"], ["vk"], "desire")
add("来たい", "来る", ["adj-i"], ["vk"], "desire")
godan(lambda u, a, i, e, o, te, ta: add(i + "たい", u, ["adj-i"], ["v5"], "desire"))

# ── volitional ─────────────────────────────────────────────────────────────────────────
add("よう", "る", ["volitional"], ["v1"], "volitional")
add("しよう", "する", ["volitional"], ["vs"], "volitional")
add("こよう", "くる", ["volitional"], ["vk"], "volitional")
add("来よう", "来る", ["volitional"], ["vk"], "volitional")
godan(lambda u, a, i, e, o, te, ta: add(o + "う", u, ["volitional"], ["v5"], "volitional"))

# ── imperative ─────────────────────────────────────────────────────────────────────────
add("ろ", "る", ["imperative"], ["v1"], "imperative")
add("よ", "る", ["imperative"], ["v1"], "imperative")
add("しろ", "する", ["imperative"], ["vs"], "imperative")
add("せよ", "する", ["imperative"], ["vs"], "imperative")
add("こい", "くる", ["imperative"], ["vk"], "imperative")
add("来い", "来る", ["imperative"], ["vk"], "imperative")
godan(lambda u, a, i, e, o, te, ta: add(e, u, ["imperative"], ["v5"], "imperative"))

# ── potential / passive / causative ────────────────────────────────────────────────────
add("られる", "る", ["v1"], ["v1"], "passive-potential")
add("れる", "る", ["v1"], ["v1"], "potential (colloquial)")
add("させる", "る", ["v1"], ["v1"], "causative")
godan(lambda u, a, i, e, o, te, ta: add(e + "る", u, ["v1"], ["v5"], "potential"))
godan(lambda u, a, i, e, o, te, ta: add(a + "れる", u, ["v1"], ["v5"], "passive"))
godan(lambda u, a, i, e, o, te, ta: add(a + "せる", u, ["v1"], ["v5"], "causative"))
# short causative (読ます) and its passive (読まされる); the -す verbs are ambiguous and left out
add("される", "す", ["v1"], ["v5"], "passive")
for u, (a, *_rest) in GODAN.items():
    if u != "す":
        add(a + "す", u, ["v5"], ["v5"], "causative (short)")

# ── て-form auxiliaries (the stem is the te-form, the auxiliary conjugates) ─────────────
add("ている", "て", ["v1"], ["te"], "progressive")
add("でいる", "で", ["v1"], ["te"], "progressive")
add("てる", "て", ["v1"], ["te"], "progressive")
add("でる", "で", ["v1"], ["te"], "progressive")
add("てしまう", "て", ["v5"], ["te"], "completive")
add("でしまう", "で", ["v5"], ["te"], "completive")
add("ちゃう", "て", ["v5"], ["te"], "completive")
add("じゃう", "で", ["v5"], ["te"], "completive")
add("てみる", "て", ["v1"], ["te"], "trial")
add("でみる", "で", ["v1"], ["te"], "trial")
add("ておく", "て", ["v5"], ["te"], "preparatory")
add("でおく", "で", ["v5"], ["te"], "preparatory")
add("ていく", "て", ["v5"], ["te"], "going")
add("でいく", "で", ["v5"], ["te"], "going")
add("てくる", "て", ["vk"], ["te"], "coming")
add("でくる", "で", ["vk"], ["te"], "coming")
add("てきた", "て", [], ["te"], "coming")

# ── appearance (そう) and nominal さ ───────────────────────────────────────────────────
add("そう", "い", ["adj-i"], ["adj-i"], "appearance")
add("そう", "る", ["adj-i"], ["v1"], "appearance")
godan(lambda u, a, i, e, o, te, ta: add(i + "そう", u, ["adj-i"], ["v5"], "appearance"))
add("さ", "い", [], ["adj-i"], "noun (-sa)")

# ── noun + する: the same する-rules, also rewriting to the bare noun (needs the dictionary's `vs`) ─
for r in [r for r in rules if r["suffixOut"] == "する" and r["suffixIn"] != "する"]:
    if r["suffixIn"].startswith(("し", "す", "さ", "せ")):
        add(r["suffixIn"], "", r["condIn"], ["vs"], r["reason"])

out = pathlib.Path(__file__).resolve().parent.parent / "extensions/lang-ja/deinflect.json"
lines = ",\n".join(json.dumps(r, ensure_ascii=False, separators=(",", ":")) for r in rules)
out.write_text('{"rules":[\n' + lines + "\n]}\n")
print(len(rules), "rules ->", out)
