"""The rules side of the agent, against the real seed and the in-memory API. No
model calls: these are the one-liner checks from development, kept."""

from pathlib import Path

import pytest

from pirtis.agent.booking import Offer, find_offer, missing_question, weekend_end
from pirtis.agent.extract import CallerInfo, calendar
from pirtis.agent.speech import day_words, number_words, offer_words, ordinal_words
from pirtis.api.memory import Memory, load_seed

SEED = Path(__file__).parents[3] / "fixtures" / "seeds" / "base.json"
TODAY = "2026-09-28"  # a Monday, as in the fixtures


def offer(**kw) -> Offer | str:
    api = Memory(load_seed(SEED))
    return find_offer(CallerInfo(**kw), api.list_units(), api, TODAY)


@pytest.mark.parametrize(
    "start,end,want",
    [
        ("2026-10-10", None, "2026-10-11"),  # Saturday alone becomes Saturday-Sunday
        ("2026-10-11", None, "2026-10-12"),  # Sunday alone becomes Sunday-Monday
        ("2026-10-06", None, "2026-10-06"),  # a weekday alone is fine
        ("2026-10-10", "2026-10-11", "2026-10-11"),  # two days are never extended
        ("2026-10-09", "2026-10-10", "2026-10-10"),
    ],
)
def test_weekend_end(start, end, want):
    assert weekend_end(start, end) == want


def test_missing_question_asks_one_thing_in_order():
    assert "dates" in missing_question(CallerInfo())
    assert "people" in missing_question(CallerInfo(start="2026-10-10"))
    assert "towing" in missing_question(CallerInfo(start="2026-10-10", people=6))
    assert missing_question(CallerInfo(start="2026-10-10", people=6, tow_kg=0)) is None


def test_too_small_unit_is_swapped_for_one_that_fits():
    o = offer(people=8, start="2026-10-03", end="2026-10-04", tow_kg=3500, unit="unit_mazoji")
    assert o.unit.id == "unit_didzioji" and o.asked_unit.id == "unit_mazoji"


def test_too_heavy_unit_is_never_offered():
    o = offer(people=6, start="2026-10-10", end="2026-10-11", tow_kg=1500, power=True)
    assert o.unit.id == "unit_vidutine"


def test_no_power_means_wood_only():
    o = offer(people=6, start="2026-10-10", end="2026-10-11", tow_kg=3000, power=False)
    assert o.unit.id == "unit_didzioji"


def test_turnaround_moves_the_dates_not_the_unit():
    o = offer(
        people=6,
        start="2026-10-03",
        end="2026-10-04",
        tow_kg=2000,
        power=True,
        unit="unit_vidutine",
    )
    assert (o.unit.id, o.start, o.end, o.taken) == (
        "unit_vidutine",
        "2026-10-04",
        "2026-10-05",
        ("2026-10-03",),
    )


def test_unknown_power_is_asked_only_when_it_matters():
    assert offer(people=6, start="2026-10-10", end="2026-10-11", tow_kg=1200) == "ask_power"
    wood = offer(people=4, start="2026-10-06", end="2026-10-07", tow_kg=2000)
    assert wood.unit.id == "unit_mazoji"


def test_nothing_fits():
    assert offer(people=10, start="2026-10-10", end="2026-10-11", tow_kg=3000) == "no_fit"


@pytest.mark.parametrize(
    "n,want",
    [
        (0, "zero"),
        (42, "forty-two"),
        (240, "two hundred and forty"),
        (1001, "one thousand and one"),
        (3500, "three thousand five hundred"),
    ],
)
def test_number_words(n, want):
    assert number_words(n) == want


def test_ordinals_and_days():
    assert [ordinal_words(n) for n in (1, 3, 12, 20, 22)] == [
        "first",
        "third",
        "twelfth",
        "twentieth",
        "twenty-second",
    ]
    assert day_words("2026-10-10") == "Saturday the tenth of October"


def test_offer_words_explain_the_change_and_use_no_digits():
    info = CallerInfo(people=4, start="2026-10-10", end="2026-10-10", tow_kg=2000)
    text = offer_words(offer(**vars(info)), info)
    assert "two days minimum" in text and "Sunday the eleventh" in text
    assert not any(ch.isdigit() for ch in text)


def test_calendar_labels_weekends():
    lines = calendar(TODAY).splitlines()
    assert lines[0] == "2026-09-28 Monday (today)"
    assert "this weekend" in lines[5] and "the weekend after next" in lines[12]
