"""O05: brochure.pdf exported by Pages into the outbox: a valid one-page A4
PDF with the brochure's text, the title drawn in a bold face clearly
larger than the body, "48 raised beds" bold; and the exported file
reopened, i.e. open in Preview when the run ends (read through the
checkers' own accessibility tool)."""
import os
import sys

sys.path.insert(0, os.path.join(os.environ.get("HAME_SUITE_DIR", os.path.dirname(__file__) + "/../.."), "fixture"))
from office import Problem, pdf_info, pdf_spans  # noqa: E402

TEXT = ["Riverside Community Garden", "Open every day from 7:00 to 19:00", "48 raised beds",
        "Workshops", "Composting basics on Saturday mornings", "Contact",
        "garden@riverside.example"]
A4 = (595.3, 841.9)


def verify_pdf(path):
    if not os.path.isfile(path):
        raise Problem(f"{os.path.basename(path)} was not exported")
    try:
        info = pdf_info(path)
        spans = pdf_spans(path)
    except Exception as e:  # noqa: BLE001 - any parse failure means an invalid PDF
        raise Problem(f"{os.path.basename(path)} is not a valid PDF: {e}") from e
    if info["pages"] != 1:
        raise Problem(f"the PDF has {info['pages']} pages, expected 1")
    if abs(info["width"] - A4[0]) > 3 or abs(info["height"] - A4[1]) > 3:
        raise Problem(f"the page is {info['width']:.0f}x{info['height']:.0f} pt, not A4 portrait")
    flat = " ".join(info["text"].split())
    missing = [t for t in TEXT if t not in flat]
    if missing:
        raise Problem(f"the PDF text lacks {missing}")
    title = [s for s in spans if "Riverside Community Garden" in s[0]]
    body = [s for s in spans if "Open every day" in s[0]]
    if not title or not body:
        raise Problem("cannot find the title and the first body line in the PDF")
    if not title[0][3] or title[0][2] < body[0][2] * 1.5:
        raise Problem(f"the title is drawn as {title[0][1]} {title[0][2]} pt; body {body[0][1]} {body[0][2]} pt")
    beds = [s for s in spans if "48 raised beds" in s[0]]
    if not beds or not beds[0][3]:
        raise Problem(f"'48 raised beds' is not bold in the PDF: {beds}")
    return "one A4 page, all text, bold larger title, bold '48 raised beds'"


def verify_reopened(path, preview_windows):
    real = os.path.realpath(path)
    docs = [os.path.realpath(w["document"]) for w in preview_windows if w.get("document")]
    if real not in docs:
        raise Problem(f"the exported PDF is not open in Preview (Preview has {docs or 'no documents'})")


def main():
    from pilot import PREVIEW, app_windows, fail, ok, strays

    if strays():
        fail(f"saved outside the requested folder: {strays()}")
    folder = os.path.join(os.environ["HAME_OUTBOX"], "o05")
    path = os.path.join(folder, "brochure.pdf")
    try:
        said = verify_pdf(path)
        verify_reopened(path, app_windows(PREVIEW))
    except Problem as e:
        fail(f"{e} (folder holds {sorted(os.listdir(folder))})")
    extra = sorted(set(os.listdir(folder)) - {"brochure.pages", "brochure.pdf"})
    if extra:
        fail(f"other files in the folder: {extra}")
    ok(f"{said}; reopened in Preview")


if __name__ == "__main__":
    main()
