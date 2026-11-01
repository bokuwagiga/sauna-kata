"""Small helpers for moving dataclasses to and from JSON-shaped dicts."""

from collections.abc import Mapping
from dataclasses import asdict, fields, is_dataclass
from typing import Any


def json_default(o: Any) -> Any:
    """json.dumps default: dataclasses become dicts, nested ones included."""
    if is_dataclass(o) and not isinstance(o, type):
        return asdict(o)
    raise TypeError(f"not JSON serializable: {type(o).__name__}")


def build[T](cls: type[T], d: Mapping[str, Any]) -> T:
    """Builds a flat dataclass from a dict, ignoring unknown keys and leaving
    missing ones at their defaults."""
    known = {f.name for f in fields(cls)}  # type: ignore[arg-type]
    return cls(**{k: v for k, v in d.items() if k in known})
