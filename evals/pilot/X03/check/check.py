"""X03: flyer.rtf edited in the workspace (new date and room, formatting
kept), then exported by TextEdit as flyer.pdf into the outbox.
Content: the workspace file and the PDF both carry the new date and room
and not the old ones. Appearance: in the file, the title is still bold,
22 pt and centred and the last line italic; in the PDF, the title is
drawn in a bold face clearly larger than the body. Output location: the
PDF is in the outbox folder (which only applications can write), named
as asked, and nothing else is there or in a default save folder."""
import os
import sys

sys.path.insert(0, os.path.join(os.environ.get("HAME_SUITE_DIR", os.path.dirname(__file__) + "/../.."), "fixture"))
from office import Problem, pdf_info, pdf_spans, rtf_paragraphs  # noqa: E402

LINES = ["Community Repair Café",
         "Saturday 29 November 2026, 10:00 to 13:00",
         "Room 2A, Riverside Library",
         "Bring one broken item per person. Volunteers fix it with you."]
OLD = ["22 November", "Room 4B"]


def verify_source(path):
    paragraphs = rtf_paragraphs(path)
    texts = [t for t, _ in paragraphs]
    if texts != LINES:
        raise Problem(f"flyer.rtf reads {texts}, expected {LINES}")
    title, last = paragraphs[0][1], paragraphs[3][1]
    if not (title["bold"] and title["size"] == 22.0 and title["align"] == "center"):
        raise Problem(f"the title lost its formatting in flyer.rtf: {title}")
    if not last["italic"]:
        raise Problem("the last line of flyer.rtf is no longer italic")
    return "flyer.rtf updated, formatting kept"


def verify_pdf(path):
    if not os.path.isfile(path):
        raise Problem("flyer.pdf was not exported to the requested folder")
    try:
        info = pdf_info(path)
        spans = pdf_spans(path)
    except Exception as e:  # noqa: BLE001 - any parse failure means an invalid PDF
        raise Problem(f"flyer.pdf is not a valid PDF: {e}") from e
    flat = " ".join(info["text"].split())
    missing = [t for t in LINES if t not in flat]
    if missing:
        raise Problem(f"the PDF lacks {missing}")
    stale = [t for t in OLD if t in flat]
    if stale:
        raise Problem(f"the PDF still says {stale}")
    title = [s for s in spans if "Community Repair" in s[0]]
    body = [s for s in spans if "Riverside Library" in s[0]]
    if not title or not body or not title[0][3] or title[0][2] < body[0][2] * 1.4:
        raise Problem(f"the PDF title is not drawn bold and larger than the body: {title[:1]} vs {body[:1]}")
    return "flyer.pdf has the new date and room, bold larger title"


def main():
    from pilot import fail, ok, strays

    if strays():
        fail(f"saved outside the requested folder: {strays()}")
    folder = os.path.join(os.environ["HAME_OUTBOX"], "x03")
    try:
        source = verify_source("flyer.rtf")
        exported = verify_pdf(os.path.join(folder, "flyer.pdf"))
    except Problem as e:
        fail(f"{e} (outbox folder holds {sorted(os.listdir(folder))})")
    extra = sorted(set(os.listdir(folder)) - {"flyer.pdf"})
    if extra:
        fail(f"other files in the folder: {extra}")
    ok(f"{source}; {exported}")


if __name__ == "__main__":
    main()
