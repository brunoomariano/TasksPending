---
version: alpha
name: TasksPending window
description: A quiet board of pending work for a Linux desktop, drawn with GTK4 CSS from the app's own palette, or from Omarchy's theme when asked.
colors:
  primary: "#7c8cff"
  on-primary: "#0e0f12"
  primary-container: "rgba(124, 140, 255, 0.14)"
  background: "#0e0f12"
  surface: "#16181d"
  surface-container: "#1c1f25"
  surface-container-high: "#23272e"
  outline: "#262a31"
  outline-variant: "#343a43"
  on-surface: "#e7e9ee"
  on-surface-variant: "#babdc5"
  on-surface-muted: "#9196a0"
  success: "#4ccb8f"
  warning: "#f2c14e"
  error: "#f2667a"
typography:
  display-clock:
    fontFamily: "bricks (drawn blocks)"
    fontSize: "fills the clamp width, 880px at most"
    fontWeight: 900
  title-group:
    fontFamily: "system-ui"
    fontSize: "0.95em"
    fontWeight: 600
  title-card:
    fontFamily: "system-ui"
    fontSize: "0.9em"
    fontWeight: 600
    lineHeight: 1.35
  body-card:
    fontFamily: "system-ui"
    fontSize: "0.85em"
    fontWeight: 400
    lineHeight: 1.4
  label-stack:
    fontFamily: "system-ui"
    fontSize: "0.78em"
    fontWeight: 600
    letterSpacing: "0.04em"
  caption:
    fontFamily: "system-ui"
    fontSize: "0.8em"
    fontWeight: 400
rounded:
  group: "12px"
  card: "10px"
  control: "8px"
  pill: "999px"
spacing:
  unit: "6px"
  card-padding: "10px 12px"
  group-padding: "12px"
  board-gutter: "28px"
  board-gap: "20px"
components:
  card:
    background: "{colors.surface-container}"
    border: "1px solid {colors.outline}"
    rounded: "{rounded.card}"
  card-hover:
    background: "{colors.surface-container-high}"
    border: "1px solid {colors.outline-variant}"
  card-marked:
    background: "{colors.primary-container}"
    border-left: "2px solid {colors.primary}"
  group:
    background: "{colors.surface}"
    border: "1px solid {colors.outline}"
    rounded: "{rounded.group}"
  button-control:
    background: "{colors.surface}"
    border: "1px solid {colors.outline}"
    rounded: "{rounded.control}"
    height: "34px"
  button-primary:
    background: "{colors.primary}"
    color: "{colors.on-primary}"
  chip-status:
    background: "tint of the state colour at 10%"
    border: "1px solid the state colour at 25%"
    rounded: "{rounded.pill}"
---

# TasksPending window

## Overview

The window is for one person at their desk, several times a day, often for a few seconds: what is waiting for me, did anything change. Personality: a quiet board. Nothing moves unless something changed, nothing is coloured unless it means something, and the only large element is the clock. Density 6 of 10 (a wall of small cards, with air between groups). Variance 3 of 10 (symmetrical, predictable, the same card everywhere). Motion 2 of 10 (short state transitions, one quiet fade for news).

Direction card, "The quiet board": the hero opens with the thing most characteristic of the subject, the time, drawn in the block font of a terminal clock, with the weather on one side and the controls on the other. Below it, two tabs: Activity, the picked cards in three stages, and the Board, one group per source, stacks inside, cards inside stacks. The one bold thing is the clock; everything else is ink on dark paper. Risk: GTK CSS has no layout, so rhythm comes from widget spacing, and no blur, so overlays are opaque surfaces with a border.

## Colors

Dark, the default:

- Page (#0e0f12): the window background, the bottom of the ladder.
- Group (#16181d): the surface of a group and of dialogs, one step up.
- Card (#1c1f25): a card, a badge, a hovered control: two steps up.
- Card hover (#23272e): a card under the pointer.
- Border (#262a31): every 1px line; Strong border (#343a43) under the pointer.
- Text (#e7e9ee), Text 2 (#babdc5) for bodies and stack names, Text 3 (#9196a0) for captions and idle controls.
- Accent (#7c8cff): the clock, the changed dot, the picked card, the tab that is open, the primary button, focus rings. Nowhere else. Text on it is the page colour (#0e0f12), and the count on the open tab is a lighter accent (#8b9aff).
- OK (#4ccb8f), Warning (#f2c14e), Danger (#f2667a): source status, attention on the Sources button, notices. Cards never carry them.

Light, when the system asks: Page (#f6f7f9), Group (#ffffff), Card (#f1f3f6), Card hover (#e8ebf0), Border (#e4e7ec), Strong border (#d3d8e0), Text (#15181e), Text 2 (#40444d), Text 3 (#626671), Accent (#4f5ee8; #4c5ade as text, #404cbb for the count on the open tab), white on it, OK (#10774e), Warning (#8d6004), Danger (#c03045).

Both palettes keep everything written as text at 4.5:1 (WCAG AA) on the grounds it is written on, by the rule the Omarchy paragraph below describes; the tests hold them to it, and the rule finds nothing left to change in them.

Omarchy, when chosen in Settings: the theme Omarchy applied, dark or light as its `colors.toml` says. Its background, text and accent are used as they are, and its green, yellow and red for the states (the window's own when it has none). The other colours are mixed from those in the same proportions as the palettes above: the surfaces and borders from the background towards the text (a light theme's group surface towards white), Text 2 and Text 3 from the text towards the background, the accent hover towards white. Text on the accent is white or the theme's dark colour (its background, or its text on a light theme), whichever has more contrast with the accent. Whatever is written in the theme's colours then reads at 4.5:1 (WCAG AA) on the grounds it is written on, as flea does it: a colour short of that is walked towards white (black on a light theme), which keeps its hue, only as far as it takes. That covers the three texts on the page, groups, cards (hovered and marked), counts and notices, the accent as text on those and on a checked tab (the accent as a fill and a frame stays as the theme has it), the count on a checked tab on its own chip (it alone goes further, so the accent elsewhere is not washed out), each state there and on its own chip, and the text on the accent. Text 2 keeps its place between Text and Text 3, so the three keep their order. A state the theme draws without hue (saturation of 0.2 or less, as on Omarchy's white, Vantablack and Solitude) cannot say what it means and, in grey, reads as switched off, so it is drawn in the theme's text colour. The window changes with the theme.

Borders carry the structure; shadows only lift the card menu and the dialogs, and never blur anything behind them.

## Typography

The system interface font (Adwaita Sans on GNOME, whatever Omarchy sets), never a downloaded one. One family; weight and size do the work: 600 for titles, 400 for bodies, 500 for buttons. Stack names are upper-case at 0.78em with 0.04em tracking, the one label style. Bodies clamp at two lines with the whole text in the tooltip. Numbers in the update line and in the controls use tabular figures.

The clock is not text: it is the "bricks" font drawn as blocks (6 wide, 5 tall, each block twice as tall as wide), in the accent colour, filling the width of the hero up to 880px.

## Layout

Hero: a centre box, weather at the start (hidden under 1440px), the clock in a clamp in the middle, the controls column at the end; 24px top, 28px sides, 20px bottom, a 1px border below. Under it, a row of two tab pills, Activity and Board; Activity carries a count pill when it holds cards. Activity: three stage columns of equal width (Picked, In Progress, Done / to mark), at least 240px, 20px apart, each a group-like box with an upper-case title and a count (Picked also has a New control at the end, for a todo), cards 8px apart, the todos after the marked cards; when nothing is picked, one line of Text 3 with a link to the Board. Board: a flow box of groups at least 290px wide, 20px apart, 28px side gutters, wrapping onto more rows when the window is narrow; a group keeps its natural height. Inside a group: a header row (logo in a tile, name, board badge, empty toggle at the end), then stacks 14px apart with a 1px rule between them; cards 8px apart.

## Elevation & Depth

Flat. Three surfaces in a ladder (page, group, card), told apart by colour and a 1px border, no shadow at rest. The card menu and the dialogs float: a border plus one soft shadow (0 8px 24px at 45% black). Pressed and hovered states change the fill, never the elevation.

## Shapes

Soft corners: 12px for groups and stage columns, dialog rows at 8px, cards at 10px, controls and the card menu at 8px, pills for badges, counts, tabs and filters. The changed dot is a 6px circle. The marked card has a 2px accent line inset on its left edge instead of a coloured background alone.

## Components

- Card: card surface, 1px border, 10px radius, 10/12px padding; title (a link when the item has one, same colour, accent on hover), two lines of body in Text 2, and one three-dot button at the end that appears on hover and stays while its menu is open. The menu holds every action: the stage moves (on Activity), send to or remove from Activity, Snooze (on the Board), Alert me about, Cancel the alert, and for a todo Edit and Delete in place of the Activity entry and Snooze. Picked: accent tint and the inset accent line. Changed: a dot before the title. Alert: a caption line in Text 2 with an alarm icon and the time.
- Tabs: pill buttons on the page with a 1px border; the open one is filled with the accent tint and outlined with the accent. Activity carries its count in a pill.
- Stage: a group-like box with the stack label style for its name and a count pill; an empty one says so in Text 3.
- Todo: a card in the Activity named after no source ("Todo" under it, no logo), written down with the New control at the end of Picked's header; its dialog is two entries, the note on what is kept in Text 3, Cancel and the one primary button. While the todos cannot be saved, the Activity opens with a stale notice saying so, over the stages.
- Stack header: a flat button with a chevron (rotated when collapsed), the upper-case name, a count pill, and the dot when a card inside changed.
- Group: group surface, 1px border, 12px radius; the logo sits in a 28px tile on the card surface.
- Controls: 34px buttons with a 1px border on the group surface; Refresh is the one filled with accent, and as wide as its longest label ("Refreshing…") whatever it says, its icon and label centred, so nothing beside it moves; the gear carries a 7px accent dot while a filter is on.
- Notices: a bordered box tinted 8% with the state colour (warning for stale data, danger for no data).
- Status chip (Sources dialog): the state colour as text on a 10% tint with a 25% ring, pill shaped.
- Dialogs: group surface, a flat header bar, 20px side padding, rows as bordered boxes; filters as pills that fill with the accent tint when active (Settings: Board, Auto-refresh, Theme, Omarchy bar on Omarchy, then the show or hide switches; under the themes, while following Omarchy, one line in Text 3 names the theme or says why there is none; under Omarchy bar, one line in Text 3 says what the icon does, what is under way or what Omarchy answered); switches in libadwaita's own style with the accent.
- Empty state: a dashed 1px border, 8px radius, a check icon and "Nothing pending." in Text 3.

## Motion

Signature: news arrives quietly. A changed dot fades in over 400ms when it appears; nothing else announces it. Hovers change colour over 120ms, the chevron rotates over 120ms, the card's menu button fades in over 120ms. No animation loops, nothing moves on its own, and the clock redraws without transition. With animations disabled by the toolkit, every state still reads from colour alone.

## Do's and Don'ts

- Do keep accent to meaning: the clock, news, picked cards, the open tab, the one primary action.
- Do use borders for structure and leave shadows to things that float.
- Do let the system font and the system's symbolic icons through; they are the integration with the desktop.
- Don't colour cards by severity, don't add counts or badges to news, don't add a header bar.
- Don't introduce a second accent, gradients, blur or decorative lines.
- Don't animate anything the user did not cause, except the fade of a new dot.
