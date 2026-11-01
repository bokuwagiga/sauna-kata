import json

from pirtis.harness.judge import verdict_schema


def test_verdict_schema():
    """The schema names one required property per criterion, carries the
    criterion text, and asks for reason before pass."""
    s = verdict_schema(["asks for the name", "does not book"])
    json.dumps(s)  # must be plain JSON
    assert s["required"] == ["criterion_1", "criterion_2"]
    c2 = s["properties"]["criterion_2"]
    assert c2["description"] == "does not book"
    assert c2["required"] == ["reason", "pass"], "want reason then pass"
    assert s["additionalProperties"] is False, "strict mode needs additionalProperties false"
