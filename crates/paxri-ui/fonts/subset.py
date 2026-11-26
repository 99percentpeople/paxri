"""Generate the terminal symbols font from Cascadia Mono PL Regular v2407.24 using fontTools."""
from pathlib import Path
import sys

from fontTools import subset
from fontTools.ttLib import TTFont

# Printable ASCII, shell punctuation, arrows, status signs, box drawing, blocks,
# Braille progress indicators, and the basic Powerline prompt separators.
RANGES = (
    (0x20, 0x7E), (0xA0, 0xA0), (0xB0, 0xB1), (0xB7, 0xB7),
    (0xD7, 0xD7), (0xF7, 0xF7), (0x2010, 0x2015), (0x2018, 0x201D),
    (0x2022, 0x2022), (0x2026, 0x2026), (0x2190, 0x2199), (0x21B5, 0x21B5),
    (0x2260, 0x2260), (0x2264, 0x2265), (0x23CE, 0x23CE), (0x2500, 0x259F),
    (0x25A0, 0x25A1), (0x25B2, 0x25B3), (0x25B6, 0x25B7), (0x25BC, 0x25BD),
    (0x25C0, 0x25C1), (0x25CB, 0x25CB), (0x25CF, 0x25CF), (0x2713, 0x2718),
    (0x2800, 0x28FF), (0xE0A0, 0xE0A3), (0xE0B0, 0xE0B3), (0xFFFD, 0xFFFD),
)

if __name__ == "__main__":
    font = TTFont(sys.argv[1], recalcTimestamp=False)
    if font["name"].getDebugName(6) != "CascadiaMonoPL-Regular":
        raise SystemExit("Expected CascadiaMonoPL-Regular.ttf")
    options = subset.Options()
    options.layout_features = []
    options.name_IDs = ["*"]
    subsetter = subset.Subsetter(options=options)
    subsetter.populate(unicodes=[cp for lo, hi in RANGES for cp in range(lo, hi + 1)])
    subsetter.subset(font)
    family = "paxri Terminal Symbols"
    names = {1: family, 3: "paxri-TerminalSymbols-1", 4: family, 6: "PaxriTerminalSymbols"}
    for record in font["name"].names:
        if record.nameID in names:
            record.string = names[record.nameID].encode(record.getEncoding())
    font.save(Path(__file__).with_name("terminal-symbols.ttf"))
