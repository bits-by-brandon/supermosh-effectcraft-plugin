# Draft: temporal input for WebAssembly plug-ins (for storytold/effectcraft)

Status: filed as two issues, since the maintainers ask for one problem per ticket:

* [storytold/effectcraft#2773](https://github.com/storytold/effectcraft/issues/2773): read the
  effect's own layer at other times (`source_at`, part 2 below)
* [storytold/effectcraft#2774](https://github.com/storytold/effectcraft/issues/2774): carry
  state from frame to frame (part 3 below)

Part 1 is [storytold/effectcraft#1977](https://github.com/storytold/effectcraft/issues/1977).
Related: [storytold/effectcraft#2572](https://github.com/storytold/effectcraft/issues/2572)
(plug-in API v2: padding and GPU kernels). The text below is the original combined draft.

---

**Title:** [Feature Request] WebAssembly plug-ins: read the layer at other times, and carry state
from frame to frame (plug-in API v2)

## Summary

Plug-in API v1 hands `ec_render` one frame of its layer and nothing else. The built-in time
effects (Echo, Time Displacement, Timewarp, Pixel Motion Blur, Remove Grain) read neighbouring
frames through `EffectHost::self_at(layer_time, effects)`, but plug-ins can't reach it. That rules
out a whole class of third-party effects: datamosh, optical-flow smears, frame blending, temporal
denoise, feedback and trails.

The concrete case is [supermosh-effectcraft-plugin](https://github.com/bits-by-brandon/supermosh-effectcraft-plugin).
Its glide works within v1 because its motion is synthetic, so any step can be worked out from the
current frame alone. A classic datamosh can't work that way. Frame N is frame N−1's *output* moved
by the motion between source frames N−1 and N, so the effect needs other source frames and its
own earlier output. Today that has to be a prepass outside EffectCraft (`tools/mvmosh` in that
repo), which loses keyframable parameters, the effect stack and live preview.

## Proposal

Additive, in a plug-in API v2 that keeps loading v1 modules unchanged (as #2572 suggests).

### 1. Declare time dependence (#1977)

An optional manifest field `"timeDependent": true`, put in the layer-cache key as
`is_time_dependent()` does for the built-in `TIME_DEPENDENT` list. Everything below implies it.

### 2. Read the layer at another time

One host import, the first in the API:

```text
(import "ec" "source_at" (func (param f64 i32 i32 i32) (result i32)))
;; source_at(layer_time, effects, dst_ptr, dst_len) -> 0 ok, 1 unavailable, 2 bad buffer
```

It writes the effect's own layer at `layer_time` into plug-in memory at `dst_ptr`: same
`width × height`, same scale and padding as the current buffer, premultiplied RGBA `f32`.
`effects` follows `self_at`: the first `effects` effects of the stack, never past this one. It is
a thin wrapper over `EffectHost::self_at`, so the host's recursion limits and caching apply
unchanged.

* **Sandbox:** the import is a pure function of the project, so plug-ins stay deterministic with
  no files, clock or network. The manifest has to declare it (`"imports": ["source_at"]`), so v1's
  "no imports" check still holds for everything else.
* **Fuel:** charge each call a budget like a frame (base + per pixel), and cap the calls per
  render (for example 64). A runaway plug-in then fails like any other over-budget render and
  leaves the frame unchanged.
* **Unavailable cases** (adjustment layers, recursion limit, the web build) return 1, and the
  plug-in decides how to degrade.

This alone covers frame blending, echo-style effects, temporal denoise and motion estimation
between neighbouring frames.

### 3. Carry state from frame to frame (recurrent effects)

Datamosh, feedback and trails need their own earlier output, and re-deriving it from `source_at`
alone makes frame N cost O(N) frames. Proposed optional exports:

```text
ec_state_len() -> i32                ;; bytes of state after the last ec_render (0 = none)
ec_state_ptr() -> i32
ec_render_recurrent(pixels, w, h, params, nparams, time, scale, prev_state_ptr, prev_state_len) -> i32
```

Before rendering frame N, the host passes the state the plug-in saved after frame N−1 (one frame
earlier in layer time, at the same scale and parameters hash). The host keeps that state in a
bounded per-instance cache. On a miss (scrubbing, a cold cache, another thread), it renders the
missing frames in order from the last cached state, or from `"recurrentStart"` (a parameter the
manifest names, such as the mosh start time), the way `ec.sim.*` effects replay from layer time 0.
Playback and sequential renders then cost O(1) per frame. Scrubbing costs at most the replay from
the nearest cached state, and results stay deterministic because the replay path and the playback
path run the same sequence.

## Alternatives considered

* **Stateful plug-ins today** (cache earlier frames in WebAssembly memory between calls): breaks
  with one instance per thread, scrubbing and preview resolution changes, and breaks v1's
  determinism contract. Not viable.
* **Prepass tools** (what `tools/mvmosh` does): work, but the effect stops being an effect: no
  keyframes, no expressions, no stacking before it, no live preview.

## Compatibility

v1 modules load and run exactly as before. A v2 module that imports nothing and exports neither
recurrent function is a v1 module with a new version number. `effect.plugins.list` could report
each plug-in's capabilities (`timeDependent`, `sourceAt`, `recurrent`).

I'm happy to prototype parts 1 and 2 against `crates/plugin/src/wasm.rs` and
`crates/effects/src/plugin.rs` if this direction sounds right.
