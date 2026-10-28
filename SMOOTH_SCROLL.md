# Experimental: smooth scroll region relay

This fork of [herdr](https://github.com/herdrdev/herdr) relays the
experimental smooth scroll region protocol from programs running in panes
to the host terminal, so pixel-smooth trackpad scrolling works inside
herdr. It is not affiliated with the herdr project and has not been
submitted upstream.

The host side is an experimental Ghostty fork:
[tlpinney/ghostty](https://github.com/tlpinney/ghostty) (branch
`smooth-scroll`, see its `SMOOTH_SCROLL.md` for the protocol). Terminals
that don't know the sequence ignore it, so this fork behaves like herdr
everywhere else.

## AI disclosure

The changes in this fork were written by
[Claude Code](https://claude.com/claude-code) using Claude Opus 5.5. A
human directed the work, chose the design, and tested it by hand on macOS
with the Ghostty fork. Herdr's commit style forbids AI co-author lines, so
the disclosure lives here instead.

## How it works

1. **Capture.** `crates/ghostty-vt` enables libghostty-vt's unknown-APC
   callback and keeps `ESC _ smooth;… ESC \` payloads. Each pane stores its
   latest declared regions (`src/smooth_scroll.rs`), cleared when the
   program switches screens.
2. **Server.** While rendering a client shell surface, herdr translates
   each visible pane's regions into pane-surface cells. Panes scrolled back
   into herdr's own scrollback contribute none. Changed regions are sent as
   the optional endpoint control `herdr.smooth_regions.v1`, framed just
   ahead of the surface in the same ordered write. Clients that don't know
   the control ignore it; no frozen codec changes.
3. **Client.** When composing a frame, the client offsets regions by the
   pane surface origin, drops any under an overlay, assigns stable host ids
   (up to 8), and writes the declarations inside the frame's synchronized
   update. Regions that go away are cleared with `smooth;off;id`.
4. **Input** needs no change: with mouse reporting on, herdr already
   forwards each host wheel event to the program as exactly one wheel
   event.

## Limitations

- Panes with declared regions skip the retained row-patch path and render
  complete surfaces, which costs more while they scroll.
- Popup panes are not relayed.
- Only one overscan row exists, so herdr's round trip and its ~60 fps
  render cap can make fast flicks pause briefly at a row boundary.
