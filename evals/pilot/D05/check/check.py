"""D05: notice.rtf saved in place by TextEdit with the title bold, 20 pt
and centred, the packing line italic, and nothing else changed.
Structure: each paragraph's text and formatting read through textutil.
Visual: the saved file rendered by Quick Look must draw the title as a
centred line clearly taller than the body lines."""
import os
import sys

sys.path.insert(0, os.path.join(os.environ.get("HAME_SUITE_DIR", os.path.dirname(__file__) + "/../.."), "fixture"))
from office import Problem, ink_bands, render, rtf_paragraphs  # noqa: E402

LINES = ["Office Move Notice",
         "Our team moves to the 5th floor on Friday, 28 November.",
         "Please pack your desk by Thursday 4 pm.",
         "IT will move computers and phones."]
WANT = [{"bold": True, "italic": False, "size": 20.0, "align": "center"},
        {"bold": False, "italic": False, "size": 12.0, "align": "left"},
        {"bold": False, "italic": True, "size": 12.0, "align": "left"},
        {"bold": False, "italic": False, "size": 12.0, "align": "left"}]


def verify_rtf(path):
    if not os.path.isfile(path):
        raise Problem("notice.rtf is gone")
    if not open(path, "rb").read(5) == b"{\\rtf":
        raise Problem("notice.rtf is no longer RTF")
    paragraphs = rtf_paragraphs(path)
    texts = [t for t, _ in paragraphs]
    if texts != LINES:
        raise Problem(f"the text now reads {texts}, expected it unchanged: {LINES}")
    for (text, got), want in zip(paragraphs, WANT):
        for key in want:
            if (got[key] if key != "align" else got[key] or "left") != want[key]:
                raise Problem(f"{text!r}: {key} is {got[key]!r}, expected {want[key]!r}")
    return "title bold 20 pt centred, packing line italic, the rest unchanged"


def verify_rendering(path):
    bands, width = ink_bands(render(path))
    if len(bands) < 4:
        raise Problem(f"the rendered notice shows {len(bands)} text lines, expected 4")
    title, body = bands[0], bands[1:]
    middle = (title[2] + title[3]) / 2
    if abs(middle - width / 2) > width * 0.06:
        raise Problem(f"the rendered title is not centred (its middle is at {middle:.0f} of {width} px)")
    body_h = sorted(b[1] - b[0] for b in body)[len(body) // 2]
    # Ink heights: body lines have descenders, the title has none.
    if (title[1] - title[0]) < 1.25 * body_h:
        raise Problem(f"the rendered title is {title[1] - title[0]} px tall, body lines {body_h} px")
    left = min(b[2] for b in body)
    if any(abs(b[2] - left) > width * 0.02 for b in body):
        raise Problem("the body lines are no longer left-aligned")
    return "rendered title centred and larger than the body"


def main():
    from pilot import fail, ok, strays

    if strays():
        fail(f"saved outside the requested folder: {strays()}")
    folder = os.path.join(os.environ["HAME_OUTBOX"], "d05")
    path = os.path.join(folder, "notice.rtf")
    try:
        said = verify_rtf(path)
        drawn = verify_rendering(path)
    except Problem as e:
        fail(str(e))
    extra = sorted(set(os.listdir(folder)) - {"notice.rtf"})
    if extra:
        fail(f"other files in the folder: {extra}")
    ok(f"{said}; {drawn}")


if __name__ == "__main__":
    main()
