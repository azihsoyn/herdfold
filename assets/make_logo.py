# Draws the logo, an open book with a ribbon and the name in a small
# serif, from the pixels below, as:
#   assets/logo.svg  each pixel a square, for the README
#   assets/logo.ans  two pixels a cell (▀ ▄), in 24-bit colour, for a terminal
#   assets/logo.txt  the same in plain text, without colour
# Run from the repository root:
#   python3 assets/make_logo.py
import html

# The colours, by the letter each pixel is drawn with.
INK = {
    '#': '#efe9da',  # page
    '=': '#9d9686',  # a line of text
    's': '#c3bba8',  # the shade by the spine
    'e': '#d9d1be',  # the edges of the pages
    'c': '#2f6f6a',  # the cover
    'd': '#26544f',  # the cover at the spine
    'r': '#e0565b',  # the ribbon
    'R': '#b8434a',  # its shaded edge
    'w': '#efe9da',  # the name
}
BACK = '#16181f'
EDGE = '#2a2e3a'
TAG_INK = '#9aa0ac'
TAGLINE = 'Long text, laid out as facing pages you turn'

# The left page; the right edge is the spine.
LEFT = [
    "    ########    ",
    "  ############  ",
    " ###############",
    " ##==========##s",
    " ##############s",
    " ##=========###s",
    " ##############s",
    " ##==========##s",
    " ##############s",
    " ##======######s",
    " ##############s",
    " e#############s",
    " eeeeeeeeeeeeees",
    "cccccccccccccccd",
]
# The right page, its text set from the left, as text is.
RIGHT = [
    "    ########    ",
    "  ############  ",
    "############### ",
    "s##==========## ",
    "s############## ",
    "s##========#### ",
    "s############## ",
    "s##==========## ",
    "s############## ",
    "s##=======##### ",
    "s############## ",
    "s#############e ",
    "seeeeeeeeeeeeee ",
    "dccccccccccccccc",
]


def book():
    rows = [' ' * 32, ' ' * 32] + [l + r for l, r in zip(LEFT, RIGHT)]
    # The ribbon: from above the book down the right page, a V cut into its
    # end, where the page shows through ('.').
    def put(y, s, x=21):
        r = list(rows[y])
        for i, ch in enumerate(s):
            if ch != '.':
                r[x + i] = ch
        rows[y] = ''.join(r)
    for y in range(8):
        put(y, 'rrR')
    put(8, 'r.R')
    return rows


# A serif face 11 pixels tall: 4 for ascenders, 7 for the x-height.
GLYPHS = {
    'h': ["###      ", " ##      ", " ##      ", " ##      ", " ## ###  ", " ###  ## ",
          " ##   ## ", " ##   ## ", " ##   ## ", " ##   ## ", "#### ####"],
    'e': ["        ", "        ", "        ", "        ", "  ####  ", " ##  ## ",
          "##    ##", "########", "##      ", " ##   ##", "  ##### "],
    'r': ["       ", "       ", "       ", "       ", "## ### ", " ###  #",
          " ##    ", " ##    ", " ##    ", " ##    ", "####   "],
    'd': ["      ###", "       ##", "       ##", "       ##", "  #### ##", " ##  ####",
          "##     ##", "##     ##", "##     ##", " ##  ####", "  #### ###"],
    'f': ["   ### ", "  ##  #", "  ##   ", "  ##   ", "#####  ", "  ##   ",
          "  ##   ", "  ##   ", "  ##   ", "  ##   ", " ####  "],
    'o': ["        ", "        ", "        ", "        ", "  ####  ", " ##  ## ",
          "##    ##", "##    ##", "##    ##", " ##  ## ", "  ####  "],
    'l': ["### ", " ## ", " ## ", " ## ", " ## ", " ## ", " ## ", " ## ", " ## ", " ## ", "####"],
}


def word(text, gap=2):
    rows = [''] * 11
    for i, c in enumerate(text):
        g = GLYPHS[c]
        w = max(len(r) for r in g)
        for y in range(11):
            rows[y] += g[y].ljust(w).replace('#', 'w') + (' ' * gap if i < len(text) - 1 else '')
    return rows


BOOK = book()
NAME = word('herdfold')
NAME_TOP = 5  # the row of the book the name's top is level with
GAP = 6       # pixels between the book and the name


def picture():
    """The book and the name side by side, as rows of pixels."""
    width = len(BOOK[0])
    rows = []
    for y in range(max(len(BOOK), NAME_TOP + len(NAME))):
        left = BOOK[y] if y < len(BOOK) else ''
        j = y - NAME_TOP
        rows.append(left.ljust(width) + ' ' * GAP + (NAME[j] if 0 <= j < len(NAME) else ''))
    return rows


def svg(px=6, pad=40):
    rows = picture()
    width = max(len(r) for r in rows) * px + pad * 2
    name_x = pad + (len(BOOK[0]) + GAP) * px
    tag_y = pad + (NAME_TOP + len(NAME)) * px + 30
    height = tag_y + 26
    rects = []
    for y, r in enumerate(rows):
        x = 0
        while x < len(r):
            ink = INK.get(r[x])
            if not ink:
                x += 1
                continue
            n = 1
            while x + n < len(r) and r[x + n] == r[x]:
                n += 1
            rects.append(f'<rect x="{pad + x * px}" y="{pad + y * px}" width="{n * px}" height="{px}" fill="{ink}"/>')
            x += n
    tag = (f'<text x="{name_x}" y="{tag_y}" font-family="Iowan Old Style, Palatino, Georgia, serif" '
           f'font-style="italic" font-size="19" fill="{TAG_INK}">{html.escape(TAGLINE)}</text>')
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" width="{width}" '
            f'height="{height}" role="img" aria-label="herdfold — {html.escape(TAGLINE.lower())}. '
            f'An open book with a ribbon in it.">\n'
            f'  <rect x="1" y="1" width="{width - 2}" height="{height - 2}" rx="14" fill="{BACK}" '
            f'stroke="{EDGE}" stroke-width="2"/>\n'
            + ''.join(f'  {r}\n' for r in rects) + f'  {tag}\n</svg>\n')


def cells(colour):
    """Two rows of pixels to a row of cells: the upper in ▀, the lower in ▄."""
    rows = picture()
    if len(rows) % 2:
        rows.append('')
    width = max(len(r) for r in rows)
    def rgb(c):
        ink = INK.get(c)
        return ink and tuple(int(ink[i:i + 2], 16) for i in (1, 3, 5))
    def on(c, row):
        # Without colour, text and shade are gaps, and so is the ribbon where
        # it lies on a page (above the book, it is drawn).
        return c in '#ecdw' or (c in 'rR' and row < 2)
    out = []
    for y in range(0, len(rows), 2):
        top, low = rows[y].ljust(width), rows[y + 1].ljust(width)
        line = ''
        for a, b in zip(top, low):
            if colour:
                ta, tb = rgb(a), rgb(b)
                if ta and tb:
                    line += '\x1b[38;2;%d;%d;%dm\x1b[48;2;%d;%d;%dm▀\x1b[0m' % (ta + tb)
                elif ta:
                    line += '\x1b[38;2;%d;%d;%dm▀\x1b[0m' % ta
                elif tb:
                    line += '\x1b[38;2;%d;%d;%dm▄\x1b[0m' % tb
                else:
                    line += ' '
            else:
                line += {(True, True): '█', (True, False): '▀', (False, True): '▄'}.get(
                    (on(a, y), on(b, y + 1)), ' ')
        out.append(line.rstrip() if not colour else line)
    tagline_at = ' ' * ((len(BOOK[0]) + GAP)) + TAGLINE
    if colour:
        tagline_at = ' ' * (len(BOOK[0]) + GAP) + '\x1b[3;38;2;154;160;172m' + TAGLINE + '\x1b[0m'
    return '\n'.join(out + ['', tagline_at]) + '\n'


open('assets/logo.svg', 'w').write(svg())
open('assets/logo.ans', 'w').write(cells(True))
open('assets/logo.txt', 'w').write(cells(False))
print('assets/logo.svg, assets/logo.ans, assets/logo.txt')
