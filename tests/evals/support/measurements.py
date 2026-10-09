"""Matched outcome and resource observations with independently known totals."""


def paired_measurements():
    slots, reviews = [], []
    for repetition in range(2):
        for condition in ("source", "portable"):
            candidate = condition == "portable"
            identifier = f"{condition}-{repetition}"
            slots.append({"review_id": identifier, "condition": condition, "repetition": repetition,
                "status": "completed", "runtime_contract": "passed", "source_integrity": "verified",
                "resources": {"input_tokens": 50 if candidate else 100, "cache_read_tokens": 10 if candidate else 20,
                    "cache_write_tokens": 0, "output_tokens": 5 if candidate else 10,
                    "run_seconds": 5 if candidate else 10, "setup_seconds": 2, "cost_usd": None, "turns": 1}})
            reviews.append({"review_id": identifier, "outcome": {"status": "satisfied" if candidate or repetition == 0 else "unsatisfied"}})
    return {"slots": slots}, {"reviewer": "fixture", "reviews": reviews}
