# Vendored crates

Permissively licensed crates carried with small patches for the Android build
(`apps/figcraft-android`, `android` branch). Each directory keeps the upstream licence files
unchanged. Remove a vendored copy once upstream releases the fix. Vendoring copyleft code is
never allowed.

| Crate | Version | Licence | Patches | From |
|---|---|---|---|---|
| winit | 0.30.13 | Apache-2.0 | **Android:** (1) each touch's tool type (finger, stylus, eraser, mouse, palm) is put in bits 32..40 of `Touch::id`, so the app can tell the S Pen from a finger and ignore a resting palm; (2) a mouse, or a touchpad in pointer mode, is delivered as cursor, button and wheel events instead of touches (hover, right-click, wheel); (3) a touchpad's two-finger swipe scrolls and its pinch zooms. **Desktop:** IME and window-placement fixes for Windows, macOS, X11 and Wayland (ports of winit#4341, #4587, #4650, #4727, #4746, #4747, #4748). Each patch is marked `PdfCraft patch:` in the source. | `Azurehawke/gridcraft` `android` branch `vendor/winit` (itself unchanged from the PdfCraft and WordCraft Android ports, where the patches are written and tested) |
| android-activity | 0.6.1 | MIT OR Apache-2.0 | One patch, marked `VectorCraft patch` in `src/native_activity/glue.rs`: the thread that runs `android_main` (and so the whole UI) gets a 64 MiB stack instead of std's 2 MiB; a stack overflow there aborts with no panic for the guard to catch. | `Azurehawke/designcraft` `android` branch `vendor/android-activity` (from VectorCraft's port, itself from the v0.6.1 tag) |

The Android patches are what `apps/figcraft-android/src/touch.rs` relies on: without them every
touch, the S Pen's included, would look like a finger.
