#!/usr/bin/env python3
"""Generates the .apkg fixtures in crates/alst-srs/tests/data/ (SPEC §11.8) with genanki (MIT).

    pip install genanki && python3 scripts/gen-anki-fixtures.py crates/alst-srs/tests/data

genanki only writes new cards, so `history.apkg` is post-processed with sqlite3 to add review state, FSRS
`data` and a revlog, following the documented legacy collection schema. Checked in; not run in CI.
"""
import json, os, shutil, sqlite3, sys, tempfile, zipfile
import genanki

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
tmp = tempfile.mkdtemp()

BASIC = genanki.Model(
    1607392319, "Basic (and reversed card)",
    fields=[{"name": "Front"}, {"name": "Back"}],
    templates=[
        {"name": "Card 1", "qfmt": "{{Front}}", "afmt": "{{FrontSide}}<hr id=answer>{{Back}}"},
        {"name": "Card 2", "qfmt": "{{Back}}", "afmt": "{{FrontSide}}<hr id=answer>{{Front}}"},
    ],
    css=".card { font-family: arial; font-size: 20px; text-align: center; }")
CLOZE = genanki.Model(
    1550428389, "Cloze",
    fields=[{"name": "Text"}, {"name": "Extra"}],
    templates=[{"name": "Cloze", "qfmt": "{{cloze:Text}}", "afmt": "{{cloze:Text}}<br>{{Extra}}"}],
    css=".cloze { font-weight: bold; color: blue; }", model_type=genanki.Model.CLOZE)

for name, data in [("cat.png", b"\x89PNG\r\n\x1a\nfake-image-bytes"), ("hola.mp3", b"ID3fake-audio-bytes")]:
    open(os.path.join(tmp, name), "wb").write(data)

def write(deck, fname, media=()):
    pkg = genanki.Package(deck)
    pkg.media_files = [os.path.join(tmp, m) for m in media]
    pkg.write_to_file(os.path.join(out, fname))

# basic.apkg: nested deck, tags, media, a reversed card, an empty-field note (one card only).
d = genanki.Deck(2059400110, "Spanish::Verbs")
d.add_note(genanki.Note(model=BASIC, fields=["hablar", "to speak"], tags=["verb", "regular"], guid="guid-hablar"))
d.add_note(genanki.Note(model=BASIC, fields=['gato<br><img src="cat.png">', "cat [sound:hola.mp3]"], tags=["noun"], guid="guid-gato"))
d.add_note(genanki.Note(model=BASIC, fields=["solo front", ""], guid="guid-solo"))
write(d, "basic.apkg", ["cat.png", "hola.mp3"])

# cloze.apkg
d = genanki.Deck(2059400111, "Geography")
d.add_note(genanki.Note(model=CLOZE, fields=["{{c1::Madrid}} is the capital of {{c2::Spain}}.", "extra"], guid="guid-capital"))
d.add_note(genanki.Note(model=CLOZE, fields=["{{c1::Paris::city}} lies on the {{c1::Seine}}.", ""], guid="guid-paris"))
write(d, "cloze.apkg")

# history.apkg: basic deck, then give some cards a schedule and a revlog.
d = genanki.Deck(2059400112, "History")
for i, (f, b) in enumerate([("uno", "one"), ("dos", "two"), ("tres", "three"), ("cuatro", "four")]):
    d.add_note(genanki.Note(model=genanki.Model(1607392320, "Basic", fields=[{"name": "Front"}, {"name": "Back"}],
        templates=[{"name": "Card 1", "qfmt": "{{Front}}", "afmt": "{{FrontSide}}<hr id=answer>{{Back}}"}]),
        fields=[f, b], guid=f"guid-h{i}"))
write(d, "history.apkg")
pk = os.path.join(out, "history.apkg")
work = tempfile.mkdtemp()
with zipfile.ZipFile(pk) as z:
    z.extractall(work)
db = sqlite3.connect(os.path.join(work, "collection.anki2"))
crt = db.execute("select crt from col").fetchone()[0]
day = lambda ts: (ts - crt) // 86400
cards = [r[0] for r in db.execute("select id from cards order by id")]
T = 1_700_000_000  # 2023-11-14 22:13:20 UTC, seconds
# card 0: review card, ivl 30d, due in 10 days; five revlog entries (learn, good, review x3, one lapse)
due0 = day(T + 10 * 86400)
db.execute("update cards set type=2, queue=2, due=?, ivl=30, factor=2500, reps=5, lapses=1 where id=?", (due0, cards[0]))
rev = [(T - 90 * 86400, 3, 0, 0, 0), (T - 89 * 86400, 3, 1, 0, 1), (T - 70 * 86400, 3, 8, 1, 1), (T - 40 * 86400, 1, -600, 8, 2), (T - 30 * 86400, 3, 30, 1, 1)]
for k, (ts, ease, ivl, last, typ) in enumerate(rev):
    db.execute("insert into revlog values (?,?,?,?,?,?,?,?,?)", (ts * 1000 + k, cards[0], -1, ease, ivl, last, 2500, 4000 + k, typ))
# card 1: FSRS memory state in data (what current Anki writes), review queue
due1 = day(T + 3 * 86400)
db.execute("update cards set type=2, queue=2, due=?, ivl=12, factor=0, reps=7, lapses=0, data=? where id=?",
           (due1, json.dumps({"pos": 2, "s": 14.5, "d": 4.2, "dr": 0.9, "decay": 0.1542}), cards[1]))
db.execute("insert into revlog values (?,?,?,?,?,?,?,?,?)", ((T - 12 * 86400) * 1000, cards[1], -1, 3, 12, 6, 0, 3000, 1))
# card 2: suspended review card, no revlog, no data
db.execute("update cards set type=2, queue=-1, due=?, ivl=5, factor=2300, reps=2, lapses=0 where id=?", (day(T + 86400), cards[2]))
# card 3 stays new
db.commit(); db.close()
shutil.copy(os.path.join(work, "collection.anki2"), os.path.join(work, "collection.anki21"))
os.remove(pk)
with zipfile.ZipFile(pk, "w", zipfile.ZIP_DEFLATED) as z:
    for n in ("collection.anki2", "collection.anki21", "media"):
        z.write(os.path.join(work, n), n)

# anki21b-only: the newer layout (compressed, protobuf), which we refuse.
with zipfile.ZipFile(os.path.join(out, "new-layout.apkg"), "w") as z:
    z.writestr("meta", b"\x08\x03")
    z.writestr("collection.anki21b", b"\x28\xb5\x2f\xfd not really zstd")
    z.writestr("media", b"\x00")
print("ok", sorted(os.listdir(out)))
