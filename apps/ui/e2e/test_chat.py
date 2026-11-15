"""End-to-end test of the web interface against a real `hame web`.

Starts the `hame` binary with a scripted model (HAME_SCRIPT, no provider is
called), opens the interface in Google Chrome through Playwright, sends a
prompt and checks: streamed text, a work group that folds with a summary,
an approval card that Enter answers, the edit on disk and the diff in the
Changes panel. Saves screenshots of the main screen to docs/design/screens.

Run from the repository root after `cargo build -p hame-cli`:

    python3 apps/ui/e2e/test_chat.py

Needs Playwright for Python and Google Chrome (`chrome` channel). Set
HAME_E2E_NO_SCREENSHOTS=1 to skip writing the screenshots.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from playwright.sync_api import expect, sync_playwright

ROOT = Path(__file__).resolve().parents[3]
BINARY = ROOT / "target" / "debug" / ("hame.exe" if os.name == "nt" else "hame")
SCREENS = ROOT / "docs" / "design" / "screens"

README = "# demo\n\nA small demo project.\n\nTODO: explain setup\n"


def text_deltas(text):
    """One delta per word, so the page has something to stream."""
    words = re.findall(r"\S+\s*|\s+", text)
    return [{"type": "text", "text": word} for word in words]


def tool(call_id, name, args):
    return [
        {"type": "tool_use_start", "id": call_id, "name": name},
        {"type": "tool_input", "json": json.dumps(args)},
    ]


def usage(inp, out):
    return {"input_tokens": inp, "output_tokens": out, "cache_read_tokens": 0, "cache_write_tokens": 0}


SCRIPT = [
    {
        "deltas": [{"type": "thinking", "text": "The user wants a setup section. Read the README first, then look for the TODO."}]
        + text_deltas("I'll read the README first and look for open TODOs.")
        + tool("c1", "read", {"path": "README.md"}),
        "stop": "tool_use",
        "usage": usage(2400, 60),
    },
    {
        "deltas": tool("c2", "grep", {"pattern": "TODO"}),
        "stop": "tool_use",
        "usage": usage(2700, 30),
    },
    {
        "deltas": tool(
            "c3",
            "edit",
            {
                "path": "README.md",
                "old_string": "TODO: explain setup\n",
                "new_string": "## Setup\n\nInstall Rust, then run `cargo build` and `cargo test`.\n",
            },
        ),
        "stop": "tool_use",
        "usage": usage(2900, 70),
    },
    {
        "deltas": text_deltas(
            "Done. The README now has a **Setup** section in place of the TODO:\n\n"
            "- how to install Rust\n- the two commands to build and test\n\n"
            "```bash\ncargo build\ncargo test\n```\n"
        ),
        "stop": "end_turn",
        "usage": usage(3100, 90),
    },
]


def start_server(tmp):
    home = tmp / "home"
    work = tmp / "demo"
    home.mkdir()
    work.mkdir()
    (work / "README.md").write_text(README)
    # Ask before every change, so the edit waits for the page's answer.
    (home / "config.toml").write_text('[approval]\ndefault = "ask"\n')
    script = tmp / "script.jsonl"
    script.write_text("".join(json.dumps(response) + "\n" for response in SCRIPT))
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.endswith("_API_KEY") and not key.startswith("HAME_")
    }
    env.update({"HAME_HOME": str(home), "HAME_SCRIPT": str(script), "HAME_SCRIPT_DELAY_MS": "35", "NO_COLOR": "1"})
    process = subprocess.Popen(
        [str(BINARY), "web", "--port", "0"],
        cwd=work,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    line = process.stdout.readline().strip()
    if not line.startswith("http://"):
        process.kill()
        raise RuntimeError(f"hame web did not print its address: {line!r} {process.stderr.read()}")
    return process, line, work


def main():
    if not BINARY.exists():
        sys.exit(f"{BINARY} is missing; run `cargo build -p hame-cli` first")
    tmp = Path(tempfile.mkdtemp(prefix="hame-e2e-"))
    process, url, work = start_server(tmp)
    shots = os.environ.get("HAME_E2E_NO_SCREENSHOTS") != "1"
    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(channel="chrome", headless=True)
            context = browser.new_context(viewport={"width": 1440, "height": 900}, color_scheme="light")
            page = context.new_page()
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(url)

            # The new-session empty state, with the token gone from the address.
            expect(page.get_by_role("heading", name="What should hame do?")).to_be_visible()
            assert "token=" not in page.url, page.url
            if shots:
                SCREENS.mkdir(parents=True, exist_ok=True)
                page.screenshot(path=str(SCREENS / "new-session-1440-light.png"))

            message = page.get_by_role("textbox", name="Message")
            message.fill("Add a setup section to the README")
            message.press("Enter")

            # The user's message, then text streaming in word by word.
            expect(page.get_by_text("Add a setup section to the README")).to_be_visible()
            streamed = page.get_by_role("log", name="Conversation").get_by_text("I'll read the README", exact=False)
            expect(streamed).to_be_visible(timeout=15000)

            # A live work group while the tools run, then the approval card
            # in place of the composer.
            approval = page.get_by_role("alertdialog")
            expect(approval).to_be_visible(timeout=20000)
            expect(approval).to_contain_text("Waiting for your approval")
            expect(approval).to_contain_text("README.md")
            expect(approval.get_by_text("Setup", exact=False).first).to_be_visible()
            expect(page.get_by_role("textbox", name="Message")).to_have_count(0)
            work_group = page.locator("section[aria-label]").filter(has=page.get_by_role("button", name=re.compile("Read README.md|Searched|Edited")))
            expect(work_group.first).to_be_visible()
            assert (work / "README.md").read_text() == README, "nothing changes before the answer"
            if shots:
                page.screenshot(path=str(SCREENS / "approval-1440-light.png"))

            # Enter allows this one call (after a short grace period, so the
            # Enter that sent a message cannot answer a question).
            page.wait_for_timeout(400)
            page.keyboard.press("Enter")
            expect(approval).to_have_count(0, timeout=10000)

            # The answer settles; the work group folds into its summary.
            expect(page.get_by_role("log", name="Conversation").get_by_text("The README now has a", exact=False)).to_be_visible(timeout=20000)
            expect(page.get_by_role("status").filter(has_text="Ready")).to_be_visible(timeout=10000)
            summary = page.get_by_role("button", name=re.compile(r"Read 1 file, searched 1 time, edited 1"))
            expect(summary).to_be_visible()
            expect(summary).to_have_attribute("aria-expanded", "false")
            expect(page.locator("pre code").filter(has_text="cargo build")).to_be_visible()
            assert "## Setup" in (work / "README.md").read_text()

            # Unfolding shows each tool row; the edit carries its diff card.
            summary.click()
            expect(page.get_by_role("button", name=re.compile("Read README.md"))).to_be_visible()
            expect(page.get_by_role("table", name="Changes to README.md").first).to_be_visible()

            # The Changes panel lists the edit.
            page.get_by_role("button", name="Show the side panel").click()
            panel = page.get_by_role("complementary", name="Side panel")
            expect(panel.get_by_role("tab", name="Changes")).to_have_attribute("aria-selected", "true")
            expect(panel.get_by_text("README.md").first).to_be_visible()

            # The session is in the list with a title from the prompt.
            expect(page.get_by_role("navigation", name="Sessions").get_by_text("Add a setup section to the README")).to_be_visible()

            # A mode change is recorded in the session and shown in the transcript.
            page.get_by_role("button", name="Close the side panel").click()
            page.get_by_role("combobox", name="Permission mode").select_option("ask")
            expect(page.get_by_role("log", name="Conversation").get_by_text("Permission mode: Ask (was Sandboxed), changed on this computer.")).to_be_visible(timeout=10000)

            # The notices of the open-source packages in the page are reachable.
            page.get_by_role("button", name="About hame and open-source licenses").click()
            link = page.get_by_role("link", name="open-source licenses")
            expect(link).to_have_attribute("href", "/licenses.txt")
            notices = page.request.get(page.url.split("#")[0].split("?")[0] + "licenses.txt")
            assert notices.ok and "highlight.js" in notices.text()
            page.keyboard.press("Escape")

            # Reloading replays the conversation from the log.
            page.reload()
            expect(page.get_by_role("log", name="Conversation").get_by_text("The README now has a", exact=False)).to_be_visible(timeout=15000)
            expect(page.get_by_role("button", name=re.compile(r"Read 1 file, searched 1 time, edited 1"))).to_be_visible()

            if shots:
                page.get_by_role("button", name="Show the side panel").click()
                page.wait_for_timeout(400)
                page.screenshot(path=str(SCREENS / "main-1440-light.png"))
                page.emulate_media(color_scheme="dark")
                page.wait_for_timeout(300)
                page.screenshot(path=str(SCREENS / "main-1440-dark.png"))
                page.emulate_media(color_scheme="light")

                phone = browser.new_context(
                    viewport={"width": 390, "height": 844},
                    device_scale_factor=3,
                    is_mobile=True,
                    has_touch=True,
                    color_scheme="light",
                )
                mobile = phone.new_page()
                # A fresh browser context has no stored token: use the printed address.
                mobile.goto(url + "#" + page.url.split("#", 1)[1])
                expect(mobile.get_by_role("log", name="Conversation").get_by_text("The README now has a", exact=False)).to_be_visible(timeout=15000)
                mobile.wait_for_timeout(400)
                mobile.screenshot(path=str(SCREENS / "main-390-light.png"))
                mobile.get_by_role("button", name="Open the session list").click()
                expect(mobile.get_by_role("navigation", name="Sessions")).to_be_visible()
                mobile.wait_for_timeout(300)
                mobile.screenshot(path=str(SCREENS / "drawer-390-light.png"))
                phone.close()

            assert not errors, errors
            browser.close()
        print("e2e: ok")
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
