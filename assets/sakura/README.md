# Sakura reference

`reference.png` is the user-approved, AI-generated Mado Home concept from this
project's design session. It is bundled as a shape and ink-intensity reference,
not displayed as a screenshot. `src/sakura.rs` samples only the tree region,
renders terminal glyphs in the live palette, and animates separate petals.
The logo and controls remain native, interactive UI.

The airborne petals adapt the independent drift, sway, tumble and halftone-dot
rendering from [Kyoto Dusk](https://ascii.rest/kyoto-dusk/). The original source
and MIT notice are retained in [`../kyoto-dusk/`](../kyoto-dusk/). Their colours
come from the active terminal theme.

Layered noise and local lighting also follow that reference: the canopy has
bright blossom clumps and shaded undersides, while bark uses finer dots. A subtle
static light wash surrounds the tree. These fields are computed once, and all
artwork fades away from the interactive controls.
