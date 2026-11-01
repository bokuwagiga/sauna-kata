import json
import re
from collections.abc import Callable
from dataclasses import dataclass, field, replace
from datetime import date, timedelta
from pathlib import Path
from typing import Any

from pirtis.api.client import (
    METHOD_BOOKINGS_CREATE,
    METHOD_BOOKINGS_LIST,
    METHOD_BOOKINGS_UPDATE,
    METHOD_CUSTOMERS_GET,
    METHOD_CUSTOMERS_IDENTIFY,
    METHOD_UNITS_AVAILABILITY,
    METHOD_UNITS_LIST,
    Call,
    Fault,
)
from pirtis.api.types import (
    CODE_BAD_DATES,
    CODE_BAD_PEOPLE,
    CODE_BAD_STATUS,
    CODE_CAPACITY_EXCEEDED,
    CODE_CUSTOMER_UNKNOWN,
    CODE_MIN_DAYS_WEEKEND,
    CODE_NOT_FOUND,
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
    Blocked,
    Booking,
    BookingPatch,
    BookingQuery,
    BookingRequest,
    Customer,
    DateRange,
    IdentifyResult,
    Unit,
    UnitAvailability,
)


@dataclass
class Seed:
    """The database contents a fixture starts from."""

    units: list[Unit] = field(default_factory=list)
    customers: list[Customer] = field(default_factory=list)
    bookings: list[Booking] = field(default_factory=list)
    blocked: list[Blocked] = field(default_factory=list)


def load_seed(path: str | Path) -> Seed:
    """Reads a seed JSON file."""
    try:
        d = json.loads(Path(path).read_text())
    except json.JSONDecodeError as e:
        raise ValueError(f"{path}: {e}") from e
    return Seed(
        units=[Unit(**u) for u in d.get("units", [])],
        customers=[Customer(**c) for c in d.get("customers", [])],
        bookings=[Booking.from_dict(b) for b in d.get("bookings", [])],
        blocked=[Blocked(**b) for b in d.get("blocked", [])],
    )


class Memory:
    """The in-memory implementation of Client.

    Records are never changed in place: bookings() hands out the stored
    objects, and the harness compares what it got before the run with what
    it gets after. update_booking builds a new record with replace().
    """

    def __init__(self, seed: Seed, faults: list[Fault] | None = None):
        # dicts keep insertion order, which is the order the seed listed them in
        self._units = {u.id: u for u in seed.units}
        self._customers = {c.id: c for c in seed.customers}
        self._bookings = {b.id: b for b in seed.bookings}
        self._blocked = list(seed.blocked)

        self._faults = list(faults or [])
        self._counts: dict[str, int] = {}
        self._calls: list[Call] = []
        self._next_id = 100

    def bookings(self) -> list[Booking]:
        """Every booking, seed and new, in insertion order. The harness reads
        it after a run. It is not part of Client and not logged."""
        return list(self._bookings.values())

    def log(self) -> list[Call]:
        return list(self._calls)

    def _call[T](self, method: str, args: Any, fn: Callable[[], T]) -> T:
        """Runs fn as one API call: injects a fault if one is due, records the
        outcome, and re-raises whatever fn raised."""
        c = Call(method=method, args=args)
        self._calls.append(c)
        try:
            fault = self._fault(method)
            if fault is not None:
                c.injected = True
                raise fault
            c.result = fn()
            return c.result
        except ApiError as e:
            c.status, c.code, c.detail = e.status, e.code, e.detail
            raise
        except Exception as e:
            c.status, c.code, c.detail = 500, "INTERNAL", str(e)
            raise

    def _fault(self, method: str) -> ApiError | None:
        """The injected error for this call of method, if any."""
        self._counts[method] = n = self._counts.get(method, 0) + 1
        for f in self._faults:
            if f.on != method or f.nth != n:
                continue
            code = f.code or {409: CODE_UNAVAILABLE, 500: "INTERNAL"}.get(f.status, "FAULT")
            return ApiError(f.status, code, "injected fault")
        return None

    def identify_customer(self, phone: str, name: str) -> IdentifyResult:
        def run() -> IdentifyResult:
            want = _digits(phone)
            given = _name_tokens(name)
            for c in self._customers.values():
                if _digits(c.phone) == want and _names_match(_name_tokens(c.name), given):
                    return IdentifyResult(ok=True, customer_id=c.id)
            return IdentifyResult(ok=False)

        return self._call(METHOD_CUSTOMERS_IDENTIFY, {"phone": phone, "name": name}, run)

    def get_customer(self, id: str) -> Customer:
        def run() -> Customer:
            c = self._customers.get(id)
            if c is None:
                raise ApiError(404, CODE_NOT_FOUND, "no customer " + id)
            return c

        return self._call(METHOD_CUSTOMERS_GET, id, run)

    def list_units(self) -> list[Unit]:
        return self._call(METHOD_UNITS_LIST, None, lambda: list(self._units.values()))

    def availability(self, q: AvailabilityQuery) -> list[UnitAvailability]:
        return self._call(METHOD_UNITS_AVAILABILITY, q, lambda: self._availability(q))

    def _availability(self, q: AvailabilityQuery) -> list[UnitAvailability]:
        start, end = _parse_range(q.from_, q.to)
        out = []
        for u in self._units.values():
            if q.people is not None and u.capacity < q.people:
                continue
            if q.max_tow_kg is not None and u.gross_weight_kg > q.max_tow_kg:
                continue
            if q.power_available is False and u.needs_power:
                continue
            busy: set[str] = set()
            for b in self._bookings.values():
                if b.unit_id != u.id or b.status != STATUS_BOOKED:
                    continue
                bs, be = _parse_range(b.start, b.end)
                # the turnaround day on either side is busy too
                busy.update(_fmt_date(d) for d in _days(bs - _DAY, be + _DAY))
            for bl in self._blocked:
                if bl.unit_id != u.id:
                    continue
                bs, be = _parse_range(bl.start, bl.end)
                busy.update(_fmt_date(d) for d in _days(bs, be))
            free: list[DateRange] = []
            cur: DateRange | None = None
            for d in _days(start, end):
                ds = _fmt_date(d)
                if ds in busy:
                    cur = None
                elif cur is None:
                    cur = DateRange(ds, ds)
                    free.append(cur)
                else:
                    cur.end = ds
            out.append(UnitAvailability(unit_id=u.id, free=free))
        return out

    def list_bookings(self, q: BookingQuery) -> list[Booking]:
        return self._call(METHOD_BOOKINGS_LIST, q, lambda: self._list_bookings(q))

    def _list_bookings(self, q: BookingQuery) -> list[Booking]:
        start = _parse_date(q.from_) if q.from_ else None
        end = _parse_date(q.to) if q.to else None
        out = []
        for b in self._bookings.values():
            if q.customer_id and b.customer_id != q.customer_id:
                continue
            if q.unit_id and b.unit_id != q.unit_id:
                continue
            if start is not None or end is not None:
                bs, be = _parse_range(b.start, b.end)
                if start is not None and be < start:
                    continue
                if end is not None and bs > end:
                    continue
            out.append(b)
        return out

    def create_booking(self, req: BookingRequest) -> Booking:
        return self._call(METHOD_BOOKINGS_CREATE, req, lambda: self._create_booking(req))

    def _create_booking(self, req: BookingRequest) -> Booking:
        self._validate(req)
        self._next_id += 1
        b = Booking(
            customer_id=req.customer_id,
            unit_id=req.unit_id,
            start=req.start,
            end=req.end,
            people=req.people,
            site=req.site,
            tow_capacity_kg=req.tow_capacity_kg,
            id=f"bk_{self._next_id}",
            version=1,
            status=STATUS_BOOKED,
            total=self._total(req),
        )
        self._bookings[b.id] = b
        return b

    def update_booking(self, id: str, version: int, patch: BookingPatch) -> Booking:
        args = {"id": id, "version": version, "patch": patch}
        return self._call(
            METHOD_BOOKINGS_UPDATE, args, lambda: self._update_booking(id, version, patch)
        )

    def _update_booking(self, id: str, version: int, patch: BookingPatch) -> Booking:
        cur = self._bookings.get(id)
        if cur is None:
            raise ApiError(404, CODE_NOT_FOUND, "no booking " + id)
        if cur.version != version:
            raise ApiError(
                409,
                CODE_VERSION_MISMATCH,
                f"booking {id} is at version {cur.version}, got {version}",
            )
        if patch.status is not None:
            if patch.status != STATUS_CANCELLED:
                raise ApiError(400, CODE_BAD_STATUS, "status can only be set to cancelled")
            cur = replace(cur, status=STATUS_CANCELLED, version=cur.version + 1)
            self._bookings[id] = cur
            return cur
        nxt = replace(
            cur,
            start=cur.start if patch.start is None else patch.start,
            end=cur.end if patch.end is None else patch.end,
            people=cur.people if patch.people is None else patch.people,
            tow_capacity_kg=(
                cur.tow_capacity_kg if patch.tow_capacity_kg is None else patch.tow_capacity_kg
            ),
        )
        self._validate(nxt, exclude_id=id)
        nxt = replace(nxt, total=self._total(nxt), version=cur.version + 1)
        self._bookings[id] = nxt
        return nxt

    def _total(self, req: BookingRequest) -> int:
        u = self._units[req.unit_id]
        try:
            s, e = _parse_range(req.start, req.end)
        except ApiError:
            return 0
        return u.price_per_day * _days_inclusive(s, e)

    def _validate(self, req: BookingRequest, exclude_id: str = "") -> None:
        """Every business rule. exclude_id skips one booking, for updates."""
        if req.customer_id not in self._customers:
            raise ApiError(400, CODE_CUSTOMER_UNKNOWN, "no customer " + req.customer_id)
        u = self._units.get(req.unit_id)
        if u is None:
            raise ApiError(400, CODE_UNIT_UNKNOWN, "no unit " + req.unit_id)
        start, end = _parse_range(req.start, req.end)
        if req.people < 1:
            raise ApiError(400, CODE_BAD_PEOPLE, "people must be at least 1")
        if req.people > u.capacity:
            raise ApiError(
                422,
                CODE_CAPACITY_EXCEEDED,
                f"{u.name} takes {u.capacity} people, asked for {req.people}",
            )
        if u.gross_weight_kg > req.tow_capacity_kg:
            raise ApiError(
                422,
                CODE_TOW_WEIGHT_EXCEEDED,
                f"{u.name} is {u.gross_weight_kg} kg, stated towing capacity is "
                f"{req.tow_capacity_kg} kg",
            )
        if u.needs_power and not req.site.power_available:
            raise ApiError(
                422, CODE_POWER_REQUIRED, f"{u.name} is electric and the site has no mains power"
            )
        if _touches_weekend(start, end) and _days_inclusive(start, end) < 2:
            raise ApiError(
                422,
                CODE_MIN_DAYS_WEEKEND,
                "rentals that touch a Saturday or Sunday are two days minimum",
            )
        for b in self._bookings.values():
            if b.id == exclude_id or b.status != STATUS_BOOKED:
                continue
            bs, be = _parse_range(b.start, b.end)
            if b.customer_id == req.customer_id and _overlaps(start, end, bs, be):
                raise ApiError(
                    422,
                    CODE_OVERLAPS_EXISTING,
                    f"customer already has {b.id} from {b.start} to {b.end}",
                )
        for b in self._bookings.values():
            if b.id == exclude_id or b.status != STATUS_BOOKED or b.unit_id != req.unit_id:
                continue
            bs, be = _parse_range(b.start, b.end)
            if _overlaps(start, end, bs, be):
                raise ApiError(
                    409, CODE_UNAVAILABLE, f"{u.name} is booked from {b.start} to {b.end}"
                )
            if _adjacent(start, end, bs, be):
                raise ApiError(
                    422,
                    CODE_TURNAROUND_CONFLICT,
                    f"{u.name} needs a clear day after the booking ending {b.end} "
                    f"or before the one starting {b.start}",
                )
        for bl in self._blocked:
            if bl.unit_id != req.unit_id:
                continue
            bs, be = _parse_range(bl.start, bl.end)
            if _overlaps(start, end, bs, be):
                raise ApiError(
                    409,
                    CODE_UNAVAILABLE,
                    f"{u.name} is out for {bl.reason} from {bl.start} to {bl.end}",
                )


# dates

_DAY = timedelta(days=1)
_DATE = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}")


def _parse_date(s: str) -> date:
    if _DATE.fullmatch(s):
        try:
            return date.fromisoformat(s)
        except ValueError:
            pass
    raise ApiError(400, CODE_BAD_DATES, f'bad date "{s}", want YYYY-MM-DD')


def _parse_range(from_: str, to: str) -> tuple[date, date]:
    f, t = _parse_date(from_), _parse_date(to)
    if t < f:
        raise ApiError(400, CODE_BAD_DATES, "end is before start")
    return f, t


def _fmt_date(d: date) -> str:
    return d.isoformat()


def _days(s: date, e: date):
    """Every date from s to e, inclusive."""
    d = s
    while d <= e:
        yield d
        d += _DAY


def _days_inclusive(s: date, e: date) -> int:
    return (e - s).days + 1


def _overlaps(a_s: date, a_e: date, b_s: date, b_e: date) -> bool:
    return a_e >= b_s and b_e >= a_s


def _adjacent(a_s: date, a_e: date, b_s: date, b_e: date) -> bool:
    """True when the ranges do not overlap but no clear day separates them."""
    return a_s == b_e + _DAY or b_s == a_e + _DAY


def _touches_weekend(s: date, e: date) -> bool:
    return any(d.weekday() >= 5 for d in _days(s, e))


# names and phones


def _digits(s: str) -> str:
    return "".join(ch for ch in s if ch.isdecimal())


def _name_tokens(s: str) -> list[str]:
    return sorted("".join(ch if ch.isalpha() else " " for ch in s.lower()).split())


def _names_match(stored: list[str], given: list[str]) -> bool:
    """Accepts the stored name exactly, or every given token present in the
    stored name when at least two distinct tokens were given. "Tomas.
    Petrauskas." from a transcript matches "Tomas Petrauskas"; "Tomas" alone
    does not, and neither does "Tomas Tomas"."""
    if not given:
        return False
    if stored == given:
        return True
    distinct = set(given)
    if len(distinct) < 2:
        return False
    return distinct <= set(stored)
