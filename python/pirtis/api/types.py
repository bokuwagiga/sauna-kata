from dataclasses import dataclass


@dataclass
class Unit:
    """One mobile sauna. The rules live in these fields."""

    id: str  # "unit_mazoji"
    name: str  # "Mažoji"
    capacity: int  # people
    gross_weight_kg: int  # what the car must tow
    heater: str  # "wood" | "electric"
    needs_power: bool  # electric units need mains at the site
    price_per_day: int


@dataclass
class Customer:
    """Who is calling, once identified. Nothing here says what their car can
    tow. Ask."""

    id: str
    name: str
    phone: str
    address: str
    past_bookings: int  # nothing here says what their car can tow. ask.


@dataclass
class IdentifyResult:
    """The outcome of matching a phone number and a spoken name."""

    ok: bool
    customer_id: str = ""


@dataclass
class AvailabilityQuery:
    """Asks which units are free between from_ and to, inclusive. The filters
    are optional. Without them every unit comes back."""

    from_: str  # "2026-10-01"
    to: str  # inclusive
    people: int | None = None  # keep units with capacity >= people
    max_tow_kg: int | None = None  # keep units with gross_weight_kg <= max_tow_kg
    power_available: bool | None = None  # False: drop units that need power


@dataclass
class DateRange:
    """An inclusive range of calendar dates."""

    start: str
    end: str


@dataclass
class UnitAvailability:
    """The free ranges of one unit. The turnaround day on either side of an
    existing booking is already removed."""

    unit_id: str
    free: list[DateRange]


@dataclass
class BookingQuery:
    """Filters bookings. Every field is optional."""

    customer_id: str = ""
    unit_id: str = ""
    from_: str = ""
    to: str = ""


@dataclass
class Site:
    """Where the sauna is going."""

    power_available: bool


@dataclass
class BookingRequest:
    """Everything the API needs to book a unit."""

    customer_id: str
    unit_id: str
    start: str  # "2026-10-03"
    end: str  # "2026-10-04", inclusive
    people: int
    site: Site
    tow_capacity_kg: int  # braked towing capacity of the car collecting it


@dataclass
class Booking(BookingRequest):
    """A stored booking."""

    id: str
    version: int
    status: str  # "booked" | "cancelled"
    total: int

    @classmethod
    def from_dict(cls, d: dict) -> "Booking":
        return cls(**{**d, "site": Site(**d["site"])})


@dataclass
class BookingPatch:
    """Changes a booking. None fields are left alone."""

    start: str | None = None
    end: str | None = None
    people: int | None = None
    tow_capacity_kg: int | None = None
    status: str | None = None  # "cancelled"


@dataclass
class Blocked:
    """Days a unit is out of service."""

    unit_id: str
    start: str
    end: str
    reason: str


# Booking statuses.
STATUS_BOOKED = "booked"
STATUS_CANCELLED = "cancelled"


class ApiError(Exception):
    """What the API raises instead of returning a result.

    400  malformed request: bad date, unknown id, people < 1. Your bug.
    404  no such record.
    409  conflict: the dates are taken, or the version is stale.
    422  rule violation: capacity, weight, power, turnaround, weekend minimum,
         overlap with the caller's own booking. The receptionist offered
         something the rules do not allow.
    """

    def __init__(self, status: int, code: str, detail: str):
        super().__init__(f"{code}: {detail}")
        self.status = status
        self.code = code
        self.detail = detail


# Reason codes.
CODE_UNAVAILABLE = "UNAVAILABLE"  # 409: the unit is booked or out of service on those dates
CODE_VERSION_MISMATCH = "VERSION_MISMATCH"  # 409: stale version on update
CODE_NOT_FOUND = "NOT_FOUND"  # 404: no such customer or booking

CODE_CUSTOMER_UNKNOWN = "CUSTOMER_UNKNOWN"  # 400
CODE_UNIT_UNKNOWN = "UNIT_UNKNOWN"  # 400
CODE_BAD_DATES = "BAD_DATES"  # 400: not YYYY-MM-DD, or end before start
CODE_BAD_PEOPLE = "BAD_PEOPLE"  # 400: people < 1
CODE_BAD_STATUS = "BAD_STATUS"  # 400: status can only be set to cancelled

CODE_CAPACITY_EXCEEDED = "CAPACITY_EXCEEDED"  # 422: people above the unit's capacity
CODE_TOW_WEIGHT_EXCEEDED = "TOW_WEIGHT_EXCEEDED"  # 422: gross_weight_kg above tow_capacity_kg
CODE_POWER_REQUIRED = "POWER_REQUIRED"  # 422: unit needs_power and the site has no mains
CODE_TURNAROUND_CONFLICT = "TURNAROUND_CONFLICT"  # 422: fewer than one clear day between bookings
CODE_MIN_DAYS_WEEKEND = "MIN_DAYS_WEEKEND"  # 422: touches Sat or Sun and is shorter than two days
CODE_OVERLAPS_EXISTING = "OVERLAPS_EXISTING"  # 422: the customer already has a booking in the range
