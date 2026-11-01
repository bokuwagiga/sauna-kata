from pirtis.agent.extract import CallerInfo


def missing_question(info: CallerInfo) -> str | None:
    """The one question to ask next for a booking, or None when we have enough to search."""
    if info.start is None:
        return "Which dates were you thinking of?"
    if info.people is None:
        return "How many people will be using it?"
    if info.tow_kg is None:
        return (
            "Just so I pick one your car can pull, "
            "do you know its braked towing capacity, in kilos?"
        )
    return None
