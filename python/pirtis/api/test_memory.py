from pathlib import Path

import pytest

from pirtis.api.client import METHOD_BOOKINGS_CREATE, Fault
from pirtis.api.memory import Memory, Seed, load_seed
from pirtis.api.types import (
    CODE_BAD_DATES,
    CODE_BAD_STATUS,
    CODE_CAPACITY_EXCEEDED,
    CODE_CUSTOMER_UNKNOWN,
    CODE_MIN_DAYS_WEEKEND,
    CODE_OVERLAPS_EXISTING,
    CODE_POWER_REQUIRED,
    CODE_TOW_WEIGHT_EXCEEDED,
    CODE_TURNAROUND_CONFLICT,
    CODE_UNAVAILABLE,
    CODE_UNIT_UNKNOWN,
    CODE_VERSION_MISMATCH,
    STATUS_BOOKED,
    STATUS_CANCELLED,
    ApiError,
    AvailabilityQuery,
    BookingPatch,
    BookingQuery,
    BookingRequest,
    Site,
)

SEED_PATH = Path(__file__).resolve().parents[3] / "fixtures" / "seeds" / "base.json"


def seed_for_test() -> Seed:
    return load_seed(SEED_PATH)


def req(cust, unit, start, end, people, tow, power) -> BookingRequest:
    return BookingRequest(
        customer_id=cust,
        unit_id=unit,
        start=start,
        end=end,
        people=people,
        site=Site(power_available=power),
        tow_capacity_kg=tow,
    )


def want_code(err: ApiError, status: int, code: str) -> None:
    assert (err.status, err.code) == (status, code), f"got {err.status} {err.code} ({err.detail})"


@pytest.mark.parametrize(
    "phone,name,ok",
    [
        ("+37061234567", "Tomas Petrauskas", True),
        ("+37061234567", "Tomas. Petrauskas.", True),
        ("+370 612 34567", "petrauskas tomas", True),
        ("+37061234567", "Tomas", False),
        ("+37061234567", "Tomas Tomas", False),
        ("+37061234567", "Jonas Jonaitis", False),
        ("+37000000000", "Tomas Petrauskas", False),
        ("+37061234567", "", False),
    ],
)
def test_identify(phone, name, ok):
    m = Memory(seed_for_test())
    assert m.identify_customer(phone, name).ok == ok


@pytest.mark.parametrize(
    "name,request_,status,code",
    [
        (
            "ok weekday",
            req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-07", 4, 1500, False),
            0,
            "",
        ),
        (
            "ok weekend two days",
            req("cus_12", "unit_mazoji", "2026-10-10", "2026-10-11", 4, 1500, False),
            0,
            "",
        ),
        (
            "capacity",
            req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-07", 5, 1500, False),
            422,
            CODE_CAPACITY_EXCEEDED,
        ),
        (
            "tow weight",
            req("cus_12", "unit_didzioji", "2026-10-10", "2026-10-11", 6, 1500, False),
            422,
            CODE_TOW_WEIGHT_EXCEEDED,
        ),
        (
            "power",
            req("cus_12", "unit_vidutine", "2026-10-10", "2026-10-11", 6, 1500, False),
            422,
            CODE_POWER_REQUIRED,
        ),
        (
            "weekend one day",
            req("cus_12", "unit_mazoji", "2026-10-10", "2026-10-10", 4, 1500, False),
            422,
            CODE_MIN_DAYS_WEEKEND,
        ),
        (
            "weekday one day ok",
            req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-06", 4, 1500, False),
            0,
            "",
        ),
        (
            "turnaround after",
            req("cus_12", "unit_vidutine", "2026-10-03", "2026-10-04", 6, 1500, True),
            422,
            CODE_TURNAROUND_CONFLICT,
        ),
        (
            "turnaround before",
            req("cus_12", "unit_didzioji", "2026-10-04", "2026-10-05", 8, 2500, True),
            422,
            CODE_TURNAROUND_CONFLICT,
        ),
        (
            "clear day ok",
            req("cus_12", "unit_vidutine", "2026-10-04", "2026-10-05", 6, 1500, True),
            0,
            "",
        ),
        (
            "unit taken",
            req("cus_12", "unit_didzioji", "2026-10-06", "2026-10-07", 8, 2500, True),
            409,
            CODE_UNAVAILABLE,
        ),
        (
            "unit blocked",
            req("cus_12", "unit_statine", "2026-10-08", "2026-10-09", 4, 1500, True),
            409,
            CODE_UNAVAILABLE,
        ),
        (
            "overlaps own",
            req("cus_07", "unit_didzioji", "2026-10-17", "2026-10-18", 8, 2500, True),
            422,
            CODE_OVERLAPS_EXISTING,
        ),
        (
            "unknown customer",
            req("cus_99", "unit_mazoji", "2026-10-06", "2026-10-07", 4, 1500, False),
            400,
            CODE_CUSTOMER_UNKNOWN,
        ),
        (
            "unknown unit",
            req("cus_12", "unit_x", "2026-10-06", "2026-10-07", 4, 1500, False),
            400,
            CODE_UNIT_UNKNOWN,
        ),
        (
            "bad dates",
            req("cus_12", "unit_mazoji", "2026-10-07", "2026-10-06", 4, 1500, False),
            400,
            CODE_BAD_DATES,
        ),
        (
            "bad date format",
            req("cus_12", "unit_mazoji", "6 October", "2026-10-07", 4, 1500, False),
            400,
            CODE_BAD_DATES,
        ),
    ],
    ids=lambda v: v if isinstance(v, str) and " " in v else None,
)
def test_rules(name, request_, status, code):
    m = Memory(seed_for_test())
    if status == 0:
        b = m.create_booking(request_)
        assert b.status == STATUS_BOOKED and b.version == 1 and b.total != 0, b
        return
    with pytest.raises(ApiError) as e:
        m.create_booking(request_)
    want_code(e.value, status, code)


def test_availability_turnaround_and_filters():
    m = Memory(seed_for_test())

    out = m.availability(AvailabilityQuery(from_="2026-10-01", to="2026-10-11"))
    by_unit = {u.unit_id: u for u in out}
    # vidutine booked Sep 30 - Oct 2, so Oct 3 is turnaround; free from Oct 4
    got = by_unit["unit_vidutine"].free
    assert len(got) == 1 and got[0].start == "2026-10-04" and got[0].end == "2026-10-11", got
    # didzioji booked Oct 6-7: free Oct 1-4 and Oct 9-11
    got = by_unit["unit_didzioji"].free
    assert len(got) == 2 and got[0].end == "2026-10-04" and got[1].start == "2026-10-09", got
    # statine blocked Oct 9
    got = by_unit["unit_statine"].free
    assert len(got) == 2 and got[0].end == "2026-10-08" and got[1].start == "2026-10-10", got

    out = m.availability(
        AvailabilityQuery(from_="2026-10-10", to="2026-10-11", people=6, power_available=False)
    )
    assert [u.unit_id for u in out] == ["unit_didzioji"], "six people, no power"

    out = m.availability(
        AvailabilityQuery(from_="2026-10-10", to="2026-10-11", people=6, max_tow_kg=1500)
    )
    assert [u.unit_id for u in out] == ["unit_vidutine"], "six people, 1500 kg"


def test_update_and_version():
    m = Memory(seed_for_test())
    b = m.create_booking(req("cus_12", "unit_mazoji", "2026-10-03", "2026-10-04", 4, 1500, False))

    with pytest.raises(ApiError) as e:
        m.update_booking(b.id, 7, BookingPatch(start="2026-10-10", end="2026-10-11"))
    want_code(e.value, 409, CODE_VERSION_MISMATCH)

    b2 = m.update_booking(b.id, b.version, BookingPatch(start="2026-10-10", end="2026-10-11"))
    assert b2.version == 2 and b2.start == "2026-10-10" and b2.total == 240, b2

    with pytest.raises(ApiError) as e:
        m.update_booking(b.id, b2.version, BookingPatch(status="booked"))
    want_code(e.value, 400, CODE_BAD_STATUS)

    b3 = m.update_booking(b.id, b2.version, BookingPatch(status=STATUS_CANCELLED))
    assert b3.status == STATUS_CANCELLED and b3.version == 3, b3
    # cancelled bookings free the unit
    m.create_booking(req("cus_07", "unit_mazoji", "2026-10-10", "2026-10-11", 4, 2000, False))


def test_faults_and_log():
    m = Memory(seed_for_test(), [Fault(on=METHOD_BOOKINGS_CREATE, nth=1, status=409)])
    r = req("cus_12", "unit_mazoji", "2026-10-06", "2026-10-07", 4, 1500, False)
    with pytest.raises(ApiError) as e:
        m.create_booking(r)
    want_code(e.value, 409, CODE_UNAVAILABLE)
    m.create_booking(r)
    log = m.log()
    assert len(log) == 2, log
    assert log[0].status == 409 and log[0].injected and log[0].method == METHOD_BOOKINGS_CREATE
    assert log[1].status == 200
    assert len(m.bookings()) == 4, "want 4 bookings (3 seed + 1)"


def test_list_bookings_filters():
    m = Memory(seed_for_test())
    out = m.list_bookings(
        BookingQuery(unit_id="unit_didzioji", from_="2026-10-06", to="2026-10-07")
    )
    assert len(out) == 1 and out[0].customer_id == "cus_31", out
    assert m.list_bookings(BookingQuery(customer_id="cus_12")) == []
