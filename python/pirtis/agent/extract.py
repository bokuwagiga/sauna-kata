from datetime import date, timedelta


def calendar(today: str, days: int = 21) -> str:
    """One line per day from today, for the model to look dates up in."""
    start_day = date.fromisoformat(today)
    calendar_lines = []
    for i in range(days):
        day = start_day + timedelta(days=i)
        calendar_lines.append(f"{day:%Y-%m-%d %A}")
    calendar_lines[0] += " (today)"
    return "\n".join(calendar_lines)
