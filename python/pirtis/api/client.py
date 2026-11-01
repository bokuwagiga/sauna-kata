"""The rental system. Three resources: customers, units, bookings. Reads
filter when asked and never enforce rules. Writes enforce every rule and raise
an ApiError with a reason code.

Do not edit this package. Build on top of it.
"""

from dataclasses import dataclass
from typing import Any, Protocol

from pirtis.api.types import (
    AvailabilityQuery,
    Booking,
    BookingPatch,
    BookingQuery,
    BookingRequest,
    Customer,
    IdentifyResult,
    Unit,
    UnitAvailability,
)


class Client(Protocol):
    """The rental API. The in-memory implementation in memory.py is what the
    harness uses; there is no HTTP."""

    def identify_customer(self, phone: str, name: str) -> IdentifyResult:
        """Matches the caller's phone number and the name they gave. ok is
        False when either does not match. There is no sign-up on the phone;
        an unknown caller is a matter for a human."""
        ...

    def get_customer(self, id: str) -> Customer:
        """Returns one customer record. 404 if unknown."""
        ...

    def list_units(self) -> list[Unit]:
        """Returns the catalogue."""
        ...

    def availability(self, q: AvailabilityQuery) -> list[UnitAvailability]:
        """Returns free date ranges per unit, with the turnaround day already
        removed. The filters are optional; without them every unit comes back
        and the model is left to pick. See AvailabilityQuery."""
        ...

    def list_bookings(self, q: BookingQuery) -> list[Booking]:
        """Returns bookings matching the query. It returns other customers'
        bookings too when the query does not name a customer."""
        ...

    def create_booking(self, req: BookingRequest) -> Booking:
        """Books a unit. It needs everything: people, whether the site has
        power, and what the collecting car can tow. ApiError with 422 and a
        reason code when a rule is violated, 409 when the dates are taken,
        400 when the request is malformed. See ApiError."""
        ...

    def update_booking(self, id: str, version: int, patch: BookingPatch) -> Booking:
        """Changes or cancels a booking. version must match the stored one,
        otherwise 409 VERSION_MISMATCH."""
        ...

    def log(self) -> list["Call"]:
        """Returns every call made so far. The harness reads it."""
        ...


@dataclass
class Call:
    """One API call as the harness sees it."""

    method: str  # "customers.get", "bookings.create", ...
    status: int = 200  # 200, 404, 409, 422, 500
    code: str = ""  # reason code when status is not 200
    detail: str = ""  # human-readable reason when status is not 200
    args: Any = None  # what was passed in
    result: Any = None  # what came back on success
    injected: bool = False  # the failure came from a Fault, not from the data


@dataclass
class Fault:
    """Injects a failure into the nth call of a method. The harness uses it to
    simulate dates taken between read and write."""

    on: str  # "bookings.create"
    nth: int  # 1 = the first call
    status: int  # 409 or 500
    code: str = ""  # defaults from status


# Method names as they appear in Call.method.
METHOD_CUSTOMERS_IDENTIFY = "customers.identify"
METHOD_CUSTOMERS_GET = "customers.get"
METHOD_UNITS_LIST = "units.list"
METHOD_UNITS_AVAILABILITY = "units.availability"
METHOD_BOOKINGS_LIST = "bookings.list"
METHOD_BOOKINGS_CREATE = "bookings.create"
METHOD_BOOKINGS_UPDATE = "bookings.update"
