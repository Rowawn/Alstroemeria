#!/usr/bin/env python3
"""Generates crates/alst-srs/tests/data/fsrs-vectors.json: FSRS-6 scheduling test vectors (SPEC §11.3).

Reference: py-fsrs (MIT), `pip install fsrs`, run with fuzzing off. The output is checked in; this script is
not run in CI. Each sequence is a list of (rating, time) answers with the reference's resulting state.
"""
import json, random, sys
from datetime import datetime, timedelta, timezone
from fsrs import Scheduler, Card, Rating, State
from fsrs.scheduler import DEFAULT_PARAMETERS

T0 = datetime(2024, 3, 1, 12, 0, tzinfo=timezone.utc)
MIN, DAY = 60_000, 86_400_000
STATE = {State.Learning: "learning", State.Review: "review", State.Relearning: "relearning"}

def ms(dt): return int((dt - T0).total_seconds() * 1000)

def run(rng, params, retention, learn, relearn, max_ivl, n):
    sch = Scheduler(parameters=params, desired_retention=retention,
                    learning_steps=[timedelta(milliseconds=m) for m in learn],
                    relearning_steps=[timedelta(milliseconds=m) for m in relearn],
                    maximum_interval=max_ivl, enable_fuzzing=False)
    card, now, events, expect = Card(), T0, [], []
    for _ in range(n):
        rating = rng.choices([Rating.Again, Rating.Hard, Rating.Good, Rating.Easy], [1, 2, 6, 2])[0]
        card, _ = sch.review_card(card, rating, now)
        events.append({"rating": int(rating), "at": ms(now)})
        expect.append({"state": STATE[card.state], "step": card.step if card.step is not None else 0,
                       "stability": card.stability, "difficulty": card.difficulty, "due": ms(card.due)})
        gap = (card.due - now) * rng.choice([0.3, 1, 1, 1, 1.5, 3])
        now = now + max(timedelta(minutes=1), gap)
    return {"retention": retention, "learning_steps": learn, "relearning_steps": relearn,
            "max_interval": max_ivl, "params": list(params), "events": events, "expect": expect}

rng = random.Random(20260301)
custom = list(DEFAULT_PARAMETERS)
custom[0], custom[2], custom[4], custom[8], custom[19], custom[20] = 0.4, 3.1, 5.5, 1.5, 0.12, 0.3
seqs = []
for i in range(40):
    seqs.append(run(rng, DEFAULT_PARAMETERS, 0.9, [MIN, 10 * MIN], [10 * MIN], 36500, 14))
for i in range(15):
    seqs.append(run(rng, custom, 0.85, [MIN, 10 * MIN, 60 * MIN], [10 * MIN, 30 * MIN], 36500, 14))
for i in range(10):
    seqs.append(run(rng, DEFAULT_PARAMETERS, 0.95, [], [], 365, 14))
for i in range(5):
    seqs.append(run(rng, DEFAULT_PARAMETERS, 0.9, [15 * MIN], [], 36500, 14))
json.dump({"reference": "py-fsrs " + __import__("importlib.metadata").metadata.version("fsrs"), "t0_utc": T0.isoformat(),
           "default_params": list(DEFAULT_PARAMETERS), "sequences": seqs}, open(sys.argv[1], "w"), separators=(",", ":"))
