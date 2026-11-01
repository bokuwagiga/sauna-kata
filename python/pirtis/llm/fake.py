from dataclasses import dataclass, field

from pirtis.llm.client import Request, Response


@dataclass
class Fake:
    """Answers from a script, then from default. It is for tests: the
    harness's own, and yours, for driving your loop through a scripted model
    without spending tokens. It does not understand anything."""

    script: list[Response] = field(default_factory=list)
    default: Response = field(default_factory=lambda: Response(content="This is the fake model."))
    i: int = 0  # model calls so far

    def chat(self, req: Request) -> Response:
        if self.i < len(self.script):
            r = self.script[self.i]
            self.i += 1
            return r
        self.i += 1
        return self.default
