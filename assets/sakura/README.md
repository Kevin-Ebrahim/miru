# Sakura reference

`reference.png` is the approved AI-generated Home concept, created when the
project was named Mado. `src/sakura.rs` samples its tree shape and brightness,
then renders glyphs in the active theme. The logo and controls are separate UI.

Petal drift, sway, tumble and dot shading follow
[Kyoto Dusk](https://ascii.rest/kyoto-dusk/). Its source and MIT notice remain in
[`../kyoto-dusk/`](../kyoto-dusk/).

The renderer precomputes blossom clusters, shaded undersides, bark texture and
light around the tree. Artwork fades near controls. Petals animate separately.
