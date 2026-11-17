"""Judge one look's screen collage with Gemini Decision 1 (typed scores, no text output).

Usage: python -I judge.py <sheet-<look>.jpg> <look> [--env <path to .env>] [--ask "<yes/no question>"]...
  look: operator | bridge (the rubric key)
The collage is the shoot.ps1 contact sheet: 2 columns, each tile labelled "<look> - <screen>".
Prints one line per criterion (0-10) and the yes-probability of each defect question.
Contract: aivory-guard/aivory-backend/services/gemini_decision.py (POST v1beta/decisions).
The API key comes from GEMINI_API_KEY or the .env file; it is never printed.
"""
import base64
import json
import os
import sys
import urllib.request

URL = "https://generativelanguage.googleapis.com/v1beta/decisions"
DEFAULT_ENV = r"C:\java\workspace\aivory-guard\aivory-backend\.env"

COMMON = {
    "readability": ("How readable is the text on the screens at full-screen size?", [
        "Most text is unreadable: too small, too faint or cut off.",
        "Much of the text is tiny or faint; several labels are hard to read.",
        "Main text readable; many small labels are tiny or faint.",
        "Text readable; a few labels are tiny or faint.",
        "All text readable; clear size steps between title, entry and label.",
        "All text crisp, with an obvious size hierarchy on every screen."]),
    "space": ("How well do the screens use their area?", [
        "Most screens are mostly empty, or content is crammed into a corner.",
        "Several screens leave half the area empty or as a blank band.",
        "Some screens have large empty bands or hollow panels.",
        "One or two screens have an empty band or a hollow panel.",
        "Content fills each screen in balance; calm empty space only.",
        "Every screen is balanced: full where it matters, calm elsewhere."]),
    "defects": ("How many visible layout defects are in the collage (overlapping text, clipped or "
                "cut text, labels cut by a frame, duplicated controls, empty boxes)?", [
        "Broken: many overlaps or clipped texts on most screens.",
        "Five or more visible defects.",
        "Three or four visible defects.",
        "Two visible defects.",
        "One visible defect.",
        "No visible defect."]),
}

RUBRICS = {
    "operator": ("Screens of one desktop app look named 'Operator'. Its goal: a retro 1999 hacker "
                 "operator station, like a dark wall of physical CRT monitors with thick bezels, dense "
                 "green falling code rain and light-blue instrument screens. The look must NOT be flat.", {
        "depth": ("How physical and three-dimensional do the panels look?", [
            "Completely flat: plain coloured rectangles with thin borders.",
            "Mostly flat; a faint border or shadow only.",
            "Some panels have a bezel or shadow, but most look like a flat web dashboard.",
            "Most panels read as screens in a frame, but the frame looks drawn and flat.",
            "Panels look like real monitors: bezel with light and shade, glass, a shadow behind.",
            "Convincing physical monitors on every screen: bezels, glass glare, depth, consistent light."]),
        "retro": ("How strongly does it evoke a realistic retro CRT operator station?", [
            "No retro feel at all: a modern generic dark UI.",
            "A green or blue colour only hints at retro.",
            "Some retro cues (mono font, green text) but it reads as a modern app.",
            "Clear retro cues (phosphor colours, mono text) but no screen glow or CRT surface.",
            "Feels like CRT screens: phosphor glow, slight vignette, scan texture, worn monitors.",
            "Immediately reads as a film-quality retro operator wall."]),
        "rain": ("How strong is the green code rain where it appears?", [
            "No code rain visible.",
            "A few sparse, dim glyphs; hard to notice.",
            "Rain is visible but sparse, dim, with no bright heads.",
            "Rain columns are clear but uneven or flat in brightness.",
            "Dense rain columns with bright glowing heads and fading tails.",
            "Dense, glowing, striking rain that still leaves text readable."]),
        "consistency": ("How consistent is the look across all screens in the collage?", [
            "Each screen looks like a different app.",
            "The main screen is styled; the others look unstyled.",
            "Shared colours, but frames and panels differ between screens.",
            "Mostly consistent; one or two screens or dialogs look plain.",
            "All screens and dialogs share one frame, palette and type.",
            "One strong, unmistakable look on every screen and dialog."]),
    }),
    "bridge": ("Screens of one desktop app look named 'Bridge', in the LCARS style of Star Trek "
               "(Okuda): black background, swept coloured frame elbows, bars that change thickness "
               "at each turn, round-capped buttons, condensed sans font, calm and never overwhelming.", {
        "frame": ("How well do the frames follow the LCARS grammar?", [
            "No LCARS frame; generic boxes.",
            "Coloured boxes, but no swept elbows or round caps.",
            "Elbows exist, but they are broken, covered or of one thickness.",
            "Correct elbows and bars on most screens; a few joins look broken.",
            "Clean swept elbows, thick-to-thin turns and round caps on every screen.",
            "Textbook LCARS frames: clean, balanced, unbroken on every screen."]),
        "calm": ("How calm and clear is each screen (LCARS is never overwhelming)?", [
            "Chaotic: everything competes for attention.",
            "Busy: many elements and colours compete.",
            "Mixed: some screens calm, others busy.",
            "Mostly calm; one or two screens are busy.",
            "Calm screens; one focal point each; colour marks state only.",
            "Serene and clear; each screen shows exactly what is needed."]),
        "navigation": ("How clear is the navigation (one Back key, a full key column, one header)?", [
            "No visible way to navigate.",
            "Navigation keys are cut, hidden or duplicated on most screens.",
            "Navigation exists but keys are missing or duplicated on several screens.",
            "Mostly clear; one screen misses or doubles a key.",
            "Clear: one header, one key column, one Back where needed.",
            "Obvious and identical navigation on every screen."]),
    }),
}


def load_key(env_path):
    if os.getenv("GEMINI_API_KEY"):
        return os.environ["GEMINI_API_KEY"]
    with open(env_path, encoding="utf-8") as f:
        for line in f:
            if line.strip().startswith("GEMINI_API_KEY="):
                return line.split("=", 1)[1].strip().strip('"').strip("'")
    sys.exit("no GEMINI_API_KEY")


def main():
    args = sys.argv[1:]
    env = DEFAULT_ENV
    if "--env" in args:
        i = args.index("--env"); env = args[i + 1]; del args[i:i + 2]
    asks = []
    while "--ask" in args:
        i = args.index("--ask"); asks.append(args[i + 1]); del args[i:i + 2]
    sheet, look = args[0], args[1]
    head, crit = RUBRICS[look]
    crit = {**crit, **COMMON}
    with open(sheet, "rb") as f:
        data = base64.b64encode(f.read()).decode()
    mime = "image/png" if sheet.lower().endswith(".png") else "image/jpeg"
    inp = [{"type": "text", "text": head + " The image is a collage: tiles in 2 columns, each tile is one "
            "full-screen screenshot labelled '<look> - <screen>' at its top left (the label is not part of "
            "the app). Judge the app only.\n"},
           {"type": "image", "data": data, "mime_type": mime, "resolution": "high"}]
    qs = {k: {"type": "score", "instructions": q, "criteria": lv} for k, (q, lv) in crit.items()}
    for n, q in enumerate(asks):
        qs[f"ask{n}"] = {"type": "noul", "instructions": q}
    body = json.dumps({"model": os.getenv("JUDGE_MODEL", "gemini-decision-1"), "input": inp,
                       "questions": qs}).encode()
    req = urllib.request.Request(URL, data=body, method="POST", headers={
        "Content-Type": "application/json", "x-goog-api-key": load_key(env)})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            payload = json.load(r)
    except urllib.error.HTTPError as e:
        sys.exit(f"HTTP {e.code}: {e.read()[:400].decode(errors='replace')}")
    ans = payload.get("answers") or {}
    total = []
    for k in crit:
        a = ans.get(k) or {}
        s = a.get("score")
        if s is None:
            print(f"{k:12} ?  {json.dumps(a)[:120]}"); continue
        total.append(float(s) * 2)
        print(f"{k:12} {float(s) * 2:4.1f}")
    if total:
        print(f"{'overall':12} {sum(total) / len(total):4.1f}")
    for n, q in enumerate(asks):
        print(f"ask{n} p(yes)={json.dumps(ans.get(f'ask{n}'))[:90]}  {q[:70]}")
    tok = (payload.get("usage") or {}).get("total_input_tokens")
    print(f"tokens {tok}")


if __name__ == "__main__":
    main()
