"""User 10-14 (feature #12): modern, flat controller prompts in place of the 360-era glossy ones.

Writes raw RGBA the game embeds (crates/skate-game/src/button_icons.rs):
  s3/<Name>.rgba  40x30, the Skate 3 menus' button textures (runtime/buttons), drawn inside 1 px so nothing clips;
  s2/<Name>.rgba  64x64 with the button in [8, 8, 40, 40], as Skate 2's bank pads it.
  ps/<Name>.rgba  40x30, a PlayStation pad's (input::pad_glyphs): named by the Xbox button in the same place.
Letters use Pillow's bundled Aileron font (SIL OFL). Run: py -3 tools/make_button_icons.py [preview.png]
"""
import os, sys
from PIL import Image, ImageDraw, ImageFont

OUT = os.path.join(os.path.dirname(__file__), "..", "crates", "skate-game", "src", "button_icons")
SS = 8  # supersampling
FACE = {"A": (16, 124, 16), "B": (196, 32, 40), "X": (20, 104, 196), "Y": (224, 160, 0)}
DARK, EDGE, INK = (34, 36, 40, 255), (150, 154, 160, 255), (245, 246, 248, 255)
# PlayStation face symbols by the Xbox button in their place: cross, circle, square, triangle.
PS_FACE = {"A": (124, 178, 232), "B": (255, 102, 102), "X": (226, 147, 216), "Y": (64, 226, 160)}
# A direction icon's unpressed arms: grey enough to show the whole cross at the menu's ~20 px.
ARM = (104, 108, 115, 255)
DIRS = {"DPadUp": (0, -1), "DPadDown": (0, 1), "DPadLeft": (-1, 0), "DPadRight": (1, 0)}
# Voice chat (user 10-26): a talker's mic lit, white on green; a silent one's grey on the sticks' dark disc.
MIC_LIT = (36, 160, 72)


def font(px):
    return ImageFont.load_default(size=px)


def text(d, box, label, px, fill):
    x0, y0, x1, y1 = box
    f = font(px)
    l, t, r, b = d.textbbox((0, 0), label, font=f, stroke_width=px // 24)
    d.text(((x0 + x1 - (r - l)) / 2 - l, (y0 + y1 - (b - t)) / 2 - t), label, font=f, fill=fill,
           stroke_width=px // 24, stroke_fill=fill)


def icon(name, w, h, ps=False):
    """The button drawn to fill a w x h box (supersampled); `ps`: a PlayStation pad's."""
    W, H = w * SS, h * SS
    im = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    s = min(W, H)
    cx, cy = W / 2, H / 2
    circle = (cx - s / 2 + SS, cy - s / 2 + SS, cx + s / 2 - SS, cy + s / 2 - SS)
    k = s / 2 - SS
    if ps and name in PS_FACE:
        d.ellipse(circle, fill=DARK, outline=EDGE, width=SS)
        c, r, wd = PS_FACE[name] + (255,), 0.46 * k, int(SS * 2.4)
        if name == "A":
            d.line((cx - r, cy - r, cx + r, cy + r), fill=c, width=wd); d.line((cx - r, cy + r, cx + r, cy - r), fill=c, width=wd)
        elif name == "B":
            d.ellipse((cx - r, cy - r, cx + r, cy + r), outline=c, width=wd)
        elif name == "X":
            d.rectangle((cx - r * 0.9, cy - r * 0.9, cx + r * 0.9, cy + r * 0.9), outline=c, width=wd)
        else:
            d.polygon([(cx, cy - r), (cx + r, cy + r * 0.75), (cx - r, cy + r * 0.75)], outline=c, width=wd)
    elif ps and name in ("L_Bumper", "R_Bumper", "L_Trigger", "R_Trigger"):
        bumper = "Bumper" in name
        box = (SS, cy - s * 0.32, W - SS, cy + s * 0.32) if bumper else (SS * 4, SS, W - SS * 4, H - SS)
        d.rounded_rectangle(box, radius=s * (0.22 if bumper else 0.3), fill=DARK, outline=EDGE, width=SS)
        text(d, box, name[0] + ("1" if bumper else "2"), int(s * 0.36), INK)
    elif ps and name in ("Back", "Start"):
        d.ellipse(circle, fill=DARK, outline=EDGE, width=SS)
        if name == "Start":  # Options: three lines
            for dy in (-0.25, 0, 0.25):
                d.line((cx - 0.38 * k, cy + dy * k, cx + 0.38 * k, cy + dy * k), fill=INK, width=int(SS * 2))
        else:  # Create: a small screen with three rays above it
            d.rounded_rectangle((cx - 0.3 * k, cy - 0.05 * k, cx + 0.3 * k, cy + 0.4 * k), radius=SS, outline=INK, width=int(SS * 1.6))
            for dx in (-0.25, 0, 0.25):
                d.line((cx + dx * k, cy - 0.45 * k, cx + dx * 1.3 * k, cy - 0.2 * k), fill=INK, width=int(SS * 1.6))
    elif ps and (name.startswith("Stick") or name.endswith("_Click")):
        click = name.endswith("_Click")
        d.ellipse(circle, fill=DARK, outline=INK if click else EDGE, width=SS * (2 if click else 1))
        side = "L" if "Left" in name or name.startswith("L3") else "R"
        text(d, circle, side + "3" if click else side, int(s * (0.36 if click else 0.46)), INK)
    elif name in FACE:
        d.ellipse(circle, fill=FACE[name] + (255,))
        text(d, circle, name, int(s * 0.62), INK)
    elif name in ("L_Bumper", "R_Bumper"):
        box = (SS, cy - s * 0.32, W - SS, cy + s * 0.32)
        d.rounded_rectangle(box, radius=s * 0.22, fill=DARK, outline=EDGE, width=SS)
        text(d, box, name[0] + "B", int(s * 0.36), INK)
    elif name in ("L_Trigger", "R_Trigger"):
        box = (SS * 4, SS, W - SS * 4, H - SS)
        d.rounded_rectangle(box, radius=s * 0.3, fill=DARK, outline=EDGE, width=SS)
        text(d, box, name[0] + "T", int(s * 0.36), INK)
    elif name in ("MicOn", "MicOff"):
        lit = name == "MicOn"
        d.ellipse(circle, fill=MIC_LIT + (255,) if lit else DARK, outline=None if lit else EDGE, width=SS)
        ink, half, lw = INK if lit else ARM, 0.17 * k, int(SS * 1.6)
        # The capsule, the holder's U round its foot, the stem and the base.
        d.rounded_rectangle((cx - half, cy - 0.58 * k, cx + half, cy + 0.08 * k), radius=half, fill=ink)
        r, held = 0.31 * k, cy - 0.08 * k
        d.arc((cx - r, held - r, cx + r, held + r), 0, 180, fill=ink, width=lw)
        d.line((cx, held + r, cx, cy + 0.42 * k), fill=ink, width=lw)
        d.line((cx - 0.22 * k, cy + 0.44 * k, cx + 0.22 * k, cy + 0.44 * k), fill=ink, width=lw)
    else:
        click = name.endswith("_Click")  # a stick click (L3 / R3): the stick with a bright double ring
        d.ellipse(circle, fill=DARK, outline=INK if click else EDGE, width=SS * (2 if click else 1))
        if click:
            d.ellipse((circle[0] + SS * 4, circle[1] + SS * 4, circle[2] - SS * 4, circle[3] - SS * 4), outline=EDGE, width=SS)
        if name == "Back":  # Xbox "View": two overlapping windows
            for dx in (-0.12, 0.12):
                q = (cx + (dx - 0.22) * k, cy + (dx - 0.18) * k, cx + (dx + 0.22) * k, cy + (dx + 0.18) * k)
                d.rounded_rectangle(q, radius=SS, fill=DARK, outline=INK, width=int(SS * 1.6))
        elif name == "Start":  # Xbox "Menu": three lines
            for dy in (-0.25, 0, 0.25):
                d.line((cx - 0.38 * k, cy + dy * k, cx + 0.38 * k, cy + dy * k), fill=INK, width=int(SS * 2))
        elif name.startswith("DPad"):
            a, b = 0.62 * k, 0.2 * k
            # A direction (DPadUp ...): the same cross, its other arms grey and the pressed arm's end lit white (user
            # 10-25: "filled in the end of the d-pad direction white to show which direction to click").
            arm = DIRS.get(name)
            cross = ARM if arm else INK
            d.rounded_rectangle((cx - a, cy - b, cx + a, cy + b), radius=SS, fill=cross)
            d.rounded_rectangle((cx - b, cy - a, cx + b, cy + a), radius=SS, fill=cross)
            if arm:
                dx, dy = arm
                ends = sorted((dx * b, dx * a)) if dx else (-b, b)
                ys = sorted((dy * b, dy * a)) if dy else (-b, b)
                d.rounded_rectangle((cx + ends[0], cy + ys[0], cx + ends[1], cy + ys[1]), radius=SS, fill=INK)
        else:  # sticks: LS / RS (L3 / R3 clicks the same)
            text(d, circle, ("L" if "Left" in name or name.startswith("L3") else "R") + "S", int(s * (0.3 if click else 0.34)), INK)
    return im.resize((w, h), Image.LANCZOS)


# Skate 3's disc set has no stick clicks; bug 10-17 #30/#39: ours (apt_scene::glyph serves them to every movie).
PLAIN = ["A", "B", "Back", "DPad", "L_Bumper", "L_Trigger", "R_Bumper", "R_Trigger", "Start", "Stick_Left", "Stick_Right", "X", "Y",
         "L3_Click", "R3_Click"]
# Bug 10-25 #21: D-pad directions (Skate 2 Gestures rows; retail's DPad_Up .. names via apt_scene::lit_arm). Voice
# chat's mics (name tags, the roster grid, the Player List, our own; button_icons::EXTRA_GLYPHS).
S3 = PLAIN + list(DIRS) + ["MicOn", "MicOff"]
# Skate 2's plain buttons only: its direction variants (DPad_Up, Stick_Left_L, ...) carry arrows past the button.
S2 = PLAIN


def main():
    sheet = []
    for game, names, size in (("s3", S3, (40, 30)), ("s2", S2, (64, 64)), ("ps", PLAIN, (40, 30))):
        os.makedirs(os.path.join(OUT, game), exist_ok=True)
        for n in names:
            if game != "s2":
                im = icon(n, 40, 28, ps=game == "ps")
                full = Image.new("RGBA", size, (0, 0, 0, 0)); full.alpha_composite(im, (0, 1))
            else:
                full = Image.new("RGBA", size, (0, 0, 0, 0)); full.alpha_composite(icon(n, 32, 32), (8, 8))
            open(os.path.join(OUT, game, n + ".rgba"), "wb").write(full.tobytes())
            sheet.append(full)
    if len(sys.argv) > 1:
        p = Image.new("RGBA", (sum(i.width * 4 for i in sheet), 256), (60, 60, 60, 255))
        x = 0
        for i in sheet:
            p.alpha_composite(i.resize((i.width * 4, i.height * 4), Image.NEAREST), (x, 0)); x += i.width * 4
        p.save(sys.argv[1])


if __name__ == "__main__":
    main()
