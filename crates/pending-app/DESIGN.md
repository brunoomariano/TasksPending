---
version: alpha
name: TasksPending window
description: A quiet board of pending work for a Linux desktop, drawn with GTK4 CSS from the app's own palette.
colors:
  primary: "#7c8cff"
  on-primary: "#ffffff"
  primary-container: "rgba(124, 140, 255, 0.14)"
  background: "#0e0f12"
  surface: "#16181d"
  surface-container: "#1c1f25"
  surface-container-high: "#23272e"
  outline: "#262a31"
  outline-variant: "#343a43"
  on-surface: "#e7e9ee"
  on-surface-variant: "#a4abb8"
  on-surface-muted: "#6d7482"
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

Direction card, "The quiet board": the hero opens with the thing most characteristic of the subject, the time, drawn in the block font of a terminal clock, with the weather on one side and the controls on the other. Below it, the board: one group per source, stacks inside, cards inside stacks. The one bold thing is the clock; everything else is ink on dark paper. Risk: GTK CSS has no layout, so rhythm comes from widget spacing, and no blur, so overlays are opaque surfaces with a border.

## Colors

Dark, the default:

- Page (#0e0f12): the window background, the bottom of the ladder.
- Group (#16181d): the surface of a group and of dialogs, one step up.
- Card (#1c1f25): a card, a badge, a hovered control: two steps up.
- Card hover (#23272e): a card under the pointer.
- Border (#262a31): every 1px line; Strong border (#343a43) under the pointer.
- Text (#e7e9ee), Text 2 (#a4abb8) for bodies and stack names, Text 3 (#6d7482) for captions and idle controls.
- Accent (#7c8cff): the clock, the changed dot, the pin when pressed, the Now row, the primary button, focus rings. Nowhere else.
- OK (#4ccb8f), Warning (#f2c14e), Danger (#f2667a): source status, attention on the Sources button, notices. Cards never carry them.

Light, when the system asks: Page (#f6f7f9), Group (#ffffff), Card (#f1f3f6), Card hover (#e8ebf0), Border (#e4e7ec), Strong border (#d3d8e0), Text (#15181e), Text 2 (#555d6b), Text 3 (#8a91a0), Accent (#4f5ee8), OK (#16a36a), Warning (#c98a06), Danger (#d9364e).

Borders carry the structure; shadows only lift the snooze popover and the dialogs, and never blur anything behind them.

## Typography

The system interface font (Adwaita Sans on GNOME, whatever Omarchy sets), never a downloaded one. One family; weight and size do the work: 600 for titles, 400 for bodies, 500 for buttons. Stack names are upper-case at 0.78em with 0.04em tracking, the one label style. Bodies clamp at two lines with the whole text in the tooltip. Numbers in the update line use tabular figures.

The clock is not text: it is the "bricks" font drawn as blocks (6 wide, 5 tall, each block twice as tall as wide), in the accent colour, filling the width of the hero up to 880px.

## Layout

Hero: a centre box, weather at the start (hidden under 1440px), the clock in a clamp in the middle, the controls column at the end; 24px top, 28px sides, 20px bottom, a 1px border below. Board: a flow box of groups at least 290px wide, 20px apart, 28px side gutters, wrapping onto more rows when the window is narrow; a group keeps its natural height. Inside a group: a header row (logo in a tile, name, board badge, empty toggle at the end), then stacks 14px apart with a 1px rule between them; cards 8px apart. The Now row is a group-like box above the board with cards in a homogeneous flow.

## Elevation & Depth

Flat. Three surfaces in a ladder (page, group, card), told apart by colour and a 1px border, no shadow at rest. The snooze popover and the dialogs float: a border plus one soft shadow (0 8px 24px at 45% black). Pressed and hovered states change the fill, never the elevation.

## Shapes

Soft corners: 12px for groups, the Now row and dialog rows at 8px, cards at 10px, controls at 8px, pills for badges, counts, filters and snooze choices. The changed dot is a 6px circle. The marked card has a 2px accent line inset on its left edge instead of a coloured background alone.

## Components

- Card: card surface, 1px border, 10px radius, 10/12px padding; title (a link when the item has one, same colour, accent on hover), two lines of body in Text 2, pin and moon buttons that appear on hover. Marked: accent tint and the inset accent line. Changed: a dot before the title.
- Stack header: a flat button with a chevron (rotated when collapsed), the upper-case name, a count pill, and the dot when a card inside changed.
- Group: group surface, 1px border, 12px radius; the logo sits in a 28px tile on the card surface.
- Controls: 34px buttons with a 1px border on the group surface; Refresh is the one filled with accent; the gear carries a 7px accent dot while a filter is on.
- Notices: a bordered box tinted 8% with the state colour (warning for stale data, danger for no data).
- Status chip (Sources dialog): the state colour as text on a 10% tint with a 25% ring, pill shaped.
- Dialogs: group surface, a flat header bar, 20px side padding, rows as bordered boxes; filters as pills that fill with the accent tint when active; switches in libadwaita's own style with the accent.
- Empty state: a dashed 1px border, 8px radius, a check icon and "Nothing pending." in Text 3.

## Motion

Signature: news arrives quietly. A changed dot fades in over 400ms when it appears; nothing else announces it. Hovers change colour over 120ms, the chevron rotates over 120ms, pins and moons fade in over 120ms. No animation loops, nothing moves on its own, and the clock redraws without transition. With animations disabled by the toolkit, every state still reads from colour alone.

## Do's and Don'ts

- Do keep accent to meaning: the clock, news, pins, Now, the one primary action.
- Do use borders for structure and leave shadows to things that float.
- Do let the system font and the system's symbolic icons through; they are the integration with the desktop.
- Don't colour cards by severity, don't add counts or badges to news, don't add a header bar.
- Don't introduce a second accent, gradients, blur or decorative lines.
- Don't animate anything the user did not cause, except the fade of a new dot.
